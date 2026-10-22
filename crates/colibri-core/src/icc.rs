//! ICC v4 display profiles, including Microsoft's `MHC2` tag.
//!
//! A calibrated display is described as a matrix/shaper profile: three
//! colorants adapted to the D50 profile connection space, one tone curve per
//! channel, the absolute luminance, the calibration curves as a `vcgt` tag for
//! classic loaders, and optionally an `MHC2` tag for Windows' GPU colour
//! pipeline.
//!
//! The MHC2 layout follows MHC2Gen (github.com/dantmnf/MHC2), whose author
//! reverse-engineered how Windows uses it. Microsoft's own documentation has
//! not been checked against this code yet, and the behaviour still has to be
//! confirmed on real Windows 11 hardware.

use crate::calibrate::{
    white_channel_amounts, CalibrateError, CalibrationCurves, Characterization, WhitePoint,
};
use crate::color::{bradford, rgb_to_xyz_matrix, Xy, Xyz, D65};
use crate::curve::TargetCurve;
use crate::math::{Mat3, Vec3};
use crate::md5::md5;
use crate::rig::Trust;

/// The ICC profile connection space illuminant, D50, as the ICC specifies it.
pub const PCS_D50: Xyz = Xyz::new(0.9642, 1.0, 0.8249);

/// sRGB primaries and white.
pub const SRGB: Primaries = Primaries {
    red: Xy::new(0.64, 0.33),
    green: Xy::new(0.30, 0.60),
    blue: Xy::new(0.15, 0.06),
    white: D65,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Primaries {
    pub red: Xy,
    pub green: Xy,
    pub blue: Xy,
    pub white: Xy,
}

impl Primaries {
    /// Linear RGB → XYZ with white at Y = 1.
    pub fn rgb_to_xyz(&self) -> Option<Mat3> {
        rgb_to_xyz_matrix(self.red, self.green, self.blue, self.white)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IccError {
    /// A value does not fit the ICC fixed-point encoding.
    OutOfRange(&'static str),
    InvalidLutSize(usize),
    Singular(&'static str),
    Malformed(&'static str),
}

impl std::fmt::Display for IccError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IccError::OutOfRange(what) => write!(f, "{what} is out of range for the ICC encoding"),
            IccError::InvalidLutSize(n) => write!(f, "MHC2 LUT size {n} is not in 2..=4096"),
            IccError::Singular(what) => write!(f, "{what} is not invertible"),
            IccError::Malformed(what) => write!(f, "malformed profile: {what}"),
        }
    }
}

impl std::error::Error for IccError {}

/// Absolute colorimetry of a display as calibrated: what each channel adds
/// at full input, and the black level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colorimetry {
    pub red: Xyz,
    pub green: Xyz,
    pub blue: Xyz,
    pub black: Xyz,
}

impl Colorimetry {
    /// Colorimetry of the display after calibrating to `white`. Needs an
    /// absolute instrument.
    pub fn calibrated(
        ch: &Characterization,
        white: WhitePoint,
    ) -> Result<Colorimetry, CalibrateError> {
        if ch.trust != Trust::Absolute {
            return Err(CalibrateError::NeedsAbsoluteInstrument);
        }
        let a = white_channel_amounts(ch, white)?;
        let col = |c: usize| {
            let v = ch.channel_matrix.column(c);
            Xyz::new(v[0] * a[c], v[1] * a[c], v[2] * a[c])
        };
        Ok(Colorimetry {
            red: col(0),
            green: col(1),
            blue: col(2),
            black: Xyz::from_vec(ch.black),
        })
    }

    /// Full white without the black level.
    pub fn white_above_black(&self) -> Xyz {
        Xyz::new(
            self.red.x + self.green.x + self.blue.x,
            self.red.y + self.green.y + self.blue.y,
            self.red.z + self.green.z + self.blue.z,
        )
    }

    /// Full white including the black level.
    pub fn white(&self) -> Xyz {
        let w = self.white_above_black();
        Xyz::new(w.x + self.black.x, w.y + self.black.y, w.z + self.black.z)
    }

    /// Linear device RGB → XYZ, normalised so device white has Y = 1.
    pub fn rgb_to_xyz(&self) -> Mat3 {
        let k = 1.0 / self.white_above_black().y;
        Mat3::from_columns(
            self.red.scaled(k).to_vec(),
            self.green.scaled(k).to_vec(),
            self.blue.scaled(k).to_vec(),
        )
    }
}

/// What the MHC2 matrix does with desktop colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mhc2Gamut {
    /// Leave colours alone; only the calibration LUTs apply. Suits Windows'
    /// Auto Color Management, which maps colours itself from the profile's
    /// primaries.
    Native,
    /// Show desktop colours as if the display had these primaries, for
    /// example sRGB on a wide-gamut panel. With `adapt_white`, the source
    /// white maps to the display's white (relative colorimetric); without
    /// it, to the source white's actual chromaticity.
    Emulate {
        source: Primaries,
        adapt_white: bool,
    },
}

/// Microsoft's `MHC2` tag: a 3x4 matrix and per-channel 1D LUTs applied by
/// the GPU after Windows' fixed SDR pipeline.
///
/// As reverse-engineered by MHC2Gen, the SDR pipeline is
/// `LUT(regamma(XYZ→sRGB · M · sRGB→XYZ · degamma(input)))`, with sRGB
/// degamma/regamma. The LUTs therefore map sRGB-encoded linear device values
/// to drive values, which is what calibration curves built for
/// [`TargetCurve::Srgb`] are.
#[derive(Clone, Debug, PartialEq)]
pub struct Mhc2 {
    /// cd/m².
    pub min_luminance: f64,
    /// cd/m².
    pub max_luminance: f64,
    /// Row-major 3x4. The fourth column is written as zero by every known tool.
    pub matrix: [[f64; 4]; 3],
    pub luts: [Vec<f64>; 3],
}

impl Mhc2 {
    /// Build the tag for a display calibrated with `curves` (which should
    /// target the sRGB tone curve, see [`Mhc2`]).
    pub fn new(
        colorimetry: &Colorimetry,
        curves: &CalibrationCurves,
        gamut: Mhc2Gamut,
        lut_size: usize,
    ) -> Result<Mhc2, IccError> {
        if !(2..=4096).contains(&lut_size) {
            return Err(IccError::InvalidLutSize(lut_size));
        }
        let m = match gamut {
            Mhc2Gamut::Native => Mat3::IDENTITY,
            Mhc2Gamut::Emulate {
                source,
                adapt_white,
            } => {
                let srgb_to_xyz = SRGB.rgb_to_xyz().ok_or(IccError::Singular("sRGB"))?;
                let xyz_to_srgb = srgb_to_xyz.inverse().ok_or(IccError::Singular("sRGB"))?;
                let src_to_xyz = source
                    .rgb_to_xyz()
                    .ok_or(IccError::Singular("source primaries"))?;
                let dev_to_xyz = colorimetry.rgb_to_xyz();
                let xyz_to_dev = dev_to_xyz
                    .inverse()
                    .ok_or(IccError::Singular("display primaries"))?;
                let adapt = if adapt_white {
                    let dev_white = Xyz::from_vec(dev_to_xyz * [1.0; 3]);
                    bradford(source.white.to_xyz(1.0), dev_white)
                } else {
                    Mat3::IDENTITY
                };
                // Windows wraps M in fixed sRGB conversions; undo them so the
                // net effect is source RGB → device RGB.
                srgb_to_xyz * xyz_to_dev * adapt * src_to_xyz * xyz_to_srgb
            }
        };
        let mut matrix = [[0.0; 4]; 3];
        for (r, row) in matrix.iter_mut().enumerate() {
            row[..3].copy_from_slice(&m.0[r]);
        }
        let samples: Vec<[f64; 3]> = (0..lut_size)
            .map(|i| {
                let x = i as f64 / (lut_size - 1) as f64;
                curves.apply([x, x, x])
            })
            .collect();
        let luts = [0, 1, 2].map(|c| samples.iter().map(|s| s[c]).collect());
        Ok(Mhc2 {
            min_luminance: colorimetry.black.y,
            max_luminance: colorimetry.white().y,
            matrix,
            luts,
        })
    }

    /// Serialise as an ICC tag element.
    pub fn to_bytes(&self) -> Result<Vec<u8>, IccError> {
        let n = self.luts[0].len();
        if !(2..=4096).contains(&n) || self.luts.iter().any(|l| l.len() != n) {
            return Err(IccError::InvalidLutSize(n));
        }
        let lut_block = 8 + 4 * n as u32;
        let mut w = Writer::default();
        w.sig(b"MHC2");
        w.u32(0);
        w.u32(n as u32);
        w.s15f16(self.min_luminance, "MHC2 minimum luminance")?;
        w.s15f16(self.max_luminance, "MHC2 maximum luminance")?;
        // Offsets from the start of the tag: header 36, matrix 48 bytes.
        w.u32(36);
        w.u32(84);
        w.u32(84 + lut_block);
        w.u32(84 + 2 * lut_block);
        for row in &self.matrix {
            for &v in row {
                w.s15f16(v, "MHC2 matrix")?;
            }
        }
        for lut in &self.luts {
            w.sig(b"sf32");
            w.u32(0);
            for &v in lut {
                w.s15f16(v, "MHC2 LUT")?;
            }
        }
        Ok(w.0)
    }

    pub fn parse(b: &[u8]) -> Result<Mhc2, IccError> {
        let r = Reader(b);
        if r.bytes(0, 4)? != b"MHC2" {
            return Err(IccError::Malformed("MHC2 signature"));
        }
        let n = r.u32(8)? as usize;
        if !(1..=4096).contains(&n) {
            return Err(IccError::InvalidLutSize(n));
        }
        let matrix_at = r.u32(20)? as usize;
        let mut matrix = [[0.0; 4]; 3];
        for (i, row) in matrix.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = r.s15f16(matrix_at + 4 * (4 * i + j))?;
            }
        }
        let lut = |k: usize| -> Result<Vec<f64>, IccError> {
            let at = r.u32(24 + 4 * k)? as usize;
            if r.bytes(at, 4)? != b"sf32" {
                return Err(IccError::Malformed("MHC2 LUT signature"));
            }
            (0..n).map(|i| r.s15f16(at + 8 + 4 * i)).collect()
        };
        Ok(Mhc2 {
            min_luminance: r.s15f16(12)?,
            max_luminance: r.s15f16(16)?,
            matrix,
            luts: [lut(0)?, lut(1)?, lut(2)?],
        })
    }

    /// The matrix's 3x3 part.
    pub fn matrix3(&self) -> Mat3 {
        let m = &self.matrix;
        Mat3([
            [m[0][0], m[0][1], m[0][2]],
            [m[1][0], m[1][1], m[1][2]],
            [m[2][0], m[2][1], m[2][2]],
        ])
    }
}

/// Everything needed to write a display profile.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayProfile {
    pub description: String,
    pub copyright: String,
    /// UTC creation time: year, month, day, hour, minute, second.
    pub created: [u16; 6],
    pub colorimetry: Colorimetry,
    /// The tone response the display was calibrated to.
    pub tone: TargetCurve,
    /// Written as `vcgt`, for loaders that use the GPU gamma ramp.
    pub calibration: Option<CalibrationCurves>,
    pub mhc2: Option<Mhc2>,
}

/// Size of each `vcgt` channel table.
const VCGT_SIZE: usize = 256;

impl DisplayProfile {
    pub fn to_bytes(&self) -> Result<Vec<u8>, IccError> {
        let white = self.colorimetry.white_above_black();
        let src_white = white.scaled(1.0 / white.y);
        let chad = bradford(src_white, PCS_D50);
        let colorant = |c: Xyz| Xyz::from_vec(chad * c.scaled(1.0 / white.y).to_vec());

        let mut tags: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"desc", mluc(&self.description)),
            (*b"cprt", mluc(&self.copyright)),
            (*b"wtpt", xyz_tag(PCS_D50)?),
            (*b"lumi", xyz_tag(self.colorimetry.white())?),
            (*b"rXYZ", xyz_tag(colorant(self.colorimetry.red))?),
            (*b"gXYZ", xyz_tag(colorant(self.colorimetry.green))?),
            (*b"bXYZ", xyz_tag(colorant(self.colorimetry.blue))?),
        ];
        let trc = para(self.tone)?;
        for sig in [b"rTRC", b"gTRC", b"bTRC"] {
            tags.push((*sig, trc.clone()));
        }
        tags.push((*b"chad", sf32(&chad)?));
        if let Some(curves) = &self.calibration {
            tags.push((*b"vcgt", vcgt(curves)));
        }
        if let Some(m) = &self.mhc2 {
            tags.push((*b"MHC2", m.to_bytes()?));
        }

        let table_len = 4 + 12 * tags.len();
        let mut offset = 128 + table_len;
        let mut table = Writer::default();
        let mut data = Writer::default();
        table.u32(tags.len() as u32);
        for (sig, bytes) in &tags {
            table.sig(sig);
            table.u32(offset as u32);
            table.u32(bytes.len() as u32);
            data.0.extend_from_slice(bytes);
            data.pad4();
            offset = 128 + table_len + data.0.len();
        }

        let size = 128 + table_len + data.0.len();
        let mut h = Writer::default();
        h.u32(size as u32);
        h.u32(0); // preferred CMM
        h.u32(0x0440_0000); // version 4.4
        h.sig(b"mntr");
        h.sig(b"RGB ");
        h.sig(b"XYZ ");
        for v in self.created {
            h.u16(v);
        }
        h.sig(b"acsp");
        h.sig(b"MSFT");
        h.u32(0); // flags
        h.u32(0); // manufacturer
        h.u32(0); // model
        h.0.extend_from_slice(&[0; 8]); // attributes
        h.u32(1); // media-relative colorimetric
        h.xyz(PCS_D50)?;
        h.sig(b"clbr"); // creator
        h.0.extend_from_slice(&[0; 16]); // profile ID, filled in below
        h.0.resize(128, 0);

        let mut out = h.0;
        out.extend(table.0);
        out.extend(data.0);
        let id = profile_id(&out);
        out[84..100].copy_from_slice(&id);
        Ok(out)
    }
}

/// ICC profile ID: MD5 of the profile with the flags, rendering intent and
/// profile ID header fields zeroed.
pub fn profile_id(profile: &[u8]) -> [u8; 16] {
    let mut p = profile.to_vec();
    p[44..48].fill(0);
    p[64..68].fill(0);
    p[84..100].fill(0);
    md5(&p)
}

/// A parsed profile: header fields and raw tag data, enough to check and
/// inspect what this module writes.
#[derive(Clone, Debug)]
pub struct ParsedProfile<'a> {
    pub bytes: &'a [u8],
    pub tags: Vec<([u8; 4], &'a [u8])>,
}

impl<'a> ParsedProfile<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<ParsedProfile<'a>, IccError> {
        let r = Reader(bytes);
        if r.u32(0)? as usize != bytes.len() {
            return Err(IccError::Malformed("size field"));
        }
        if r.bytes(36, 4)? != b"acsp" {
            return Err(IccError::Malformed("acsp signature"));
        }
        let count = r.u32(128)? as usize;
        let mut tags = Vec::with_capacity(count);
        for i in 0..count {
            let at = 132 + 12 * i;
            let sig: [u8; 4] = r.bytes(at, 4)?.try_into().unwrap();
            let offset = r.u32(at + 4)? as usize;
            let size = r.u32(at + 8)? as usize;
            if offset % 4 != 0 {
                return Err(IccError::Malformed("tag offset not 4-byte aligned"));
            }
            tags.push((sig, r.bytes(offset, size)?));
        }
        Ok(ParsedProfile { bytes, tags })
    }

    pub fn tag(&self, sig: &[u8; 4]) -> Option<&'a [u8]> {
        self.tags.iter().find(|(s, _)| s == sig).map(|(_, d)| *d)
    }

    pub fn xyz(&self, sig: &[u8; 4]) -> Result<Xyz, IccError> {
        let r = Reader(self.tag(sig).ok_or(IccError::Malformed("missing tag"))?);
        if r.bytes(0, 4)? != b"XYZ " {
            return Err(IccError::Malformed("XYZ type"));
        }
        Ok(Xyz::new(r.s15f16(8)?, r.s15f16(12)?, r.s15f16(16)?))
    }

    pub fn profile_id_matches(&self) -> bool {
        self.bytes[84..100] == profile_id(self.bytes)
    }
}

fn mluc(text: &str) -> Vec<u8> {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let mut w = Writer::default();
    w.sig(b"mluc");
    w.u32(0);
    w.u32(1); // records
    w.u32(12); // record size
    w.sig(b"enUS");
    w.u32(2 * utf16.len() as u32);
    w.u32(28); // string offset from tag start
    for c in utf16 {
        w.u16(c);
    }
    w.0
}

fn xyz_tag(v: Xyz) -> Result<Vec<u8>, IccError> {
    let mut w = Writer::default();
    w.sig(b"XYZ ");
    w.u32(0);
    w.xyz(v)?;
    Ok(w.0)
}

fn sf32(m: &Mat3) -> Result<Vec<u8>, IccError> {
    let mut w = Writer::default();
    w.sig(b"sf32");
    w.u32(0);
    for row in &m.0 {
        for &v in row {
            w.s15f16(v, "sf32 value")?;
        }
    }
    Ok(w.0)
}

/// Parametric curve: type 0 (pure gamma) or type 3 (sRGB form).
fn para(curve: TargetCurve) -> Result<Vec<u8>, IccError> {
    let mut w = Writer::default();
    w.sig(b"para");
    w.u32(0);
    let params: Vec<f64> = match curve {
        TargetCurve::Gamma(g) => {
            w.u16(0);
            vec![g]
        }
        TargetCurve::Srgb => {
            w.u16(3);
            vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]
        }
    };
    w.u16(0);
    for p in params {
        w.s15f16(p, "parametric curve parameter")?;
    }
    Ok(w.0)
}

/// Apple's `vcgt` tag in table form: 3 channels of 16-bit entries.
fn vcgt(curves: &CalibrationCurves) -> Vec<u8> {
    let mut w = Writer::default();
    w.sig(b"vcgt");
    w.u32(0);
    w.u32(0); // table
    w.u16(3);
    w.u16(VCGT_SIZE as u16);
    w.u16(2);
    let samples: Vec<Vec3> = (0..VCGT_SIZE)
        .map(|i| {
            let x = i as f64 / (VCGT_SIZE - 1) as f64;
            curves.apply([x, x, x])
        })
        .collect();
    for c in 0..3 {
        for s in &samples {
            w.u16((s[c].clamp(0.0, 1.0) * 65535.0).round() as u16);
        }
    }
    w.0
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn sig(&mut self, s: &[u8; 4]) {
        self.0.extend_from_slice(s);
    }
    fn s15f16(&mut self, v: f64, what: &'static str) -> Result<(), IccError> {
        self.0
            .extend_from_slice(&encode_s15f16(v, what)?.to_be_bytes());
        Ok(())
    }
    fn xyz(&mut self, v: Xyz) -> Result<(), IccError> {
        self.s15f16(v.x, "XYZ value")?;
        self.s15f16(v.y, "XYZ value")?;
        self.s15f16(v.z, "XYZ value")
    }
    fn pad4(&mut self) {
        while self.0.len() % 4 != 0 {
            self.0.push(0);
        }
    }
}

fn encode_s15f16(v: f64, what: &'static str) -> Result<i32, IccError> {
    if !v.is_finite() || !(-32768.0..32768.0).contains(&v) {
        return Err(IccError::OutOfRange(what));
    }
    Ok((v * 65536.0).round() as i32)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&self, at: usize, len: usize) -> Result<&'a [u8], IccError> {
        self.0
            .get(at..at.checked_add(len).ok_or(IccError::Malformed("offset"))?)
            .ok_or(IccError::Malformed("data out of bounds"))
    }
    fn u32(&self, at: usize) -> Result<u32, IccError> {
        Ok(u32::from_be_bytes(self.bytes(at, 4)?.try_into().unwrap()))
    }
    fn s15f16(&self, at: usize) -> Result<f64, IccError> {
        Ok(i32::from_be_bytes(self.bytes(at, 4)?.try_into().unwrap()) as f64 / 65536.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colorimetry() -> Colorimetry {
        // A display with sRGB primaries and D65 white at 100 cd/m², 0.1 black.
        let m = SRGB.rgb_to_xyz().unwrap();
        let col = |c: usize| Xyz::from_vec(m.column(c)).scaled(100.0);
        Colorimetry {
            red: col(0),
            green: col(1),
            blue: col(2),
            black: D65.to_xyz(0.1),
        }
    }

    fn profile(mhc2: Option<Mhc2>) -> DisplayProfile {
        DisplayProfile {
            description: "colibri test".into(),
            copyright: "No copyright".into(),
            created: [2026, 10, 22, 12, 0, 0],
            colorimetry: colorimetry(),
            tone: TargetCurve::Srgb,
            calibration: Some(CalibrationCurves::identity(1024)),
            mhc2,
        }
    }

    #[test]
    fn s15f16_encoding() {
        assert_eq!(encode_s15f16(1.0, "").unwrap(), 0x0001_0000);
        assert_eq!(encode_s15f16(-1.0, "").unwrap() as u32, 0xFFFF_0000);
        assert_eq!(encode_s15f16(0.5, "").unwrap(), 0x0000_8000);
        assert!(encode_s15f16(32768.0, "").is_err());
        assert!(encode_s15f16(f64::NAN, "").is_err());
    }

    #[test]
    fn profile_structure_is_valid() {
        let bytes = profile(None).to_bytes().unwrap();
        assert_eq!(bytes.len() % 4, 0);
        let p = ParsedProfile::parse(&bytes).unwrap();
        assert_eq!(&bytes[8..12], &[4, 0x40, 0, 0]);
        assert_eq!(&bytes[12..24], b"mntrRGB XYZ ");
        assert!(p.profile_id_matches());
        for sig in [
            b"desc", b"cprt", b"wtpt", b"lumi", b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"chad",
            b"vcgt",
        ] {
            assert!(p.tag(sig).is_some(), "{}", String::from_utf8_lossy(sig));
        }
        assert!(p.tag(b"MHC2").is_none());
    }

    #[test]
    fn colorants_sum_to_d50() {
        let bytes = profile(None).to_bytes().unwrap();
        let p = ParsedProfile::parse(&bytes).unwrap();
        let [r, g, b] = [b"rXYZ", b"gXYZ", b"bXYZ"].map(|s| p.xyz(s).unwrap());
        // Each value is rounded to 1/65536, so allow a little slack.
        assert!((r.x + g.x + b.x - PCS_D50.x).abs() < 1e-4);
        assert!((r.y + g.y + b.y - 1.0).abs() < 1e-4);
        assert!((r.z + g.z + b.z - PCS_D50.z).abs() < 1e-4);
        let lumi = p.xyz(b"lumi").unwrap();
        assert!((lumi.y - 100.1).abs() < 1e-3);
    }

    #[test]
    fn mhc2_layout_matches_mhc2gen() {
        let c = colorimetry();
        let m = Mhc2::new(&c, &CalibrationCurves::identity(16), Mhc2Gamut::Native, 16).unwrap();
        let b = m.to_bytes().unwrap();
        assert_eq!(b.len(), 108 + 12 * 16);
        let r = Reader(&b);
        assert_eq!(r.u32(8).unwrap(), 16);
        assert_eq!(r.u32(20).unwrap(), 36);
        assert_eq!(r.u32(24).unwrap(), 84);
        assert_eq!(r.u32(28).unwrap(), 84 + 8 + 4 * 16);
        assert_eq!(r.u32(32).unwrap(), 84 + 2 * (8 + 4 * 16));
        let back = Mhc2::parse(&b).unwrap();
        assert!(back.matrix3().max_abs_diff(&Mat3::IDENTITY) < 1e-4);
        assert!((back.luts[1][15] - 1.0).abs() < 1e-4 && back.luts[2][0].abs() < 1e-4);
        assert!((back.max_luminance - 100.1).abs() < 1e-3);
    }

    #[test]
    fn mhc2_rejects_bad_lut_sizes() {
        let c = colorimetry();
        let id = CalibrationCurves::identity(8);
        assert_eq!(
            Mhc2::new(&c, &id, Mhc2Gamut::Native, 1),
            Err(IccError::InvalidLutSize(1))
        );
        assert_eq!(
            Mhc2::new(&c, &id, Mhc2Gamut::Native, 4097),
            Err(IccError::InvalidLutSize(4097))
        );
    }

    #[test]
    fn emulating_own_primaries_is_identity() {
        let c = colorimetry();
        let gamut = Mhc2Gamut::Emulate {
            source: SRGB,
            adapt_white: true,
        };
        let m = Mhc2::new(&c, &CalibrationCurves::identity(8), gamut, 8).unwrap();
        assert!(
            m.matrix3().max_abs_diff(&Mat3::IDENTITY) < 1e-9,
            "{:?}",
            m.matrix
        );
    }

    #[test]
    fn profile_with_mhc2_round_trips() {
        let c = colorimetry();
        let m = Mhc2::new(
            &c,
            &CalibrationCurves::identity(1024),
            Mhc2Gamut::Native,
            1024,
        )
        .unwrap();
        let bytes = profile(Some(m.clone())).to_bytes().unwrap();
        let p = ParsedProfile::parse(&bytes).unwrap();
        let back = Mhc2::parse(p.tag(b"MHC2").unwrap()).unwrap();
        assert_eq!(back.luts[0].len(), 1024);
        assert!(p.profile_id_matches());
    }
}
