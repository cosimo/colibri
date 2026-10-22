# colibri

A modern, open display calibration and profiling tool that works with the
colorimeters people already own, and can also do useful work with just a
phone or camera.

Status: design stage. See `research/` for the experiments behind the design.

## Why

The current options are a vendor app that is locked to one brand of sensor and
gets abandoned, or ArgyllCMS (excellent engine, CLI only) with DisplayCAL on top
(powerful but dated and complicated). Neither is the tool most people want:
plug in a sensor or pick up a phone, press one button, get a correct profile
installed, and get told in plain words how good the display now is.

## Two ways to measure, and what each one can do

A display calibration pipeline needs to know two kinds of things:

| What                                        | Kind     | Colorimeter | Phone/camera (RAW) |
|---------------------------------------------|----------|-------------|--------------------|
| Per-channel tone curves (gamma)             | relative | yes         | **yes, exactly**   |
| Neutral grays (gray tracking at native white) | relative | yes         | **yes, exactly**   |
| Screen uniformity across the panel          | relative | slow, one point at a time | **yes, whole panel in one shot** |
| Absolute white point (e.g. hit D65)          | absolute | yes         | no, only with a reference |
| Primaries / gamut, ICC colorant matrix       | absolute | yes         | no, only with a reference |
| Absolute luminance (cd/m²)                   | absolute | yes         | rough at best      |

### Why a camera can do the relative half exactly

A camera is not a colorimeter. Its RGB sensitivities are not a linear transform
of the human colour matching functions, so a single RAW→XYZ matrix is only
correct for the light source it was fitted on.

A display, though, only ever emits mixtures of three fixed spectra:

```
spectrum(r, g, b) = R(r)·P_red + G(g)·P_green + B(b)·P_blue
```

RAW sensor values are linear in the spectrum, so for **this one display**:

```
camera_raw = M_cam · [R(r), G(g), B(b)]       (M_cam is 3x3)
```

Photograph full red, full green and full blue: those three RAW triplets are the
columns of `M_cam`. Invert it and every later photo gives the display's
*linear* channel outputs directly, without knowing anything about the camera.
That is enough to measure each channel's tone curve and build 1D calibration
curves that make every gray a scaled copy of white.

Example (`python3 research/camera_gray_tracking.py`): a simulated wide-gamut
panel with mismatched per-channel gammas (2.05 / 2.30 / 2.55), measured with a
simulated phone camera with 0.2% noise:

```
A. Camera as colorimeter (matrix fitted on another panel type)
   white error  dE00 = 12.26   (a decent colorimeter: < 1)

B. Camera as relative instrument (no camera characterization)
   level  gamma(meas)  dE00 vs ideal gray   | uncalibrated dE00
      64       2.211           0.21         |       9.22
     128       2.206           0.23         |       7.72
     192       2.199           0.12         |       4.55
```

Treating the camera as a colorimeter gets the white point badly wrong. Treating
it as a relative instrument calibrates gamma and gray balance to well under
1 ΔE00, which is about the limit of visibility.

### Getting absolute numbers out of camera mode

Camera mode can still say something about the white point and gamut if it has
a reference:

1. **EDID**: every monitor reports nominal primary and white chromaticities.
   These are often generic factory values, so use them as a prior, not as truth.
2. **Borrowed colorimeter**: measure once with a real sensor to get a
   correction matrix for this camera and this panel (the same idea as
   ArgyllCMS `.ccmx`). Store it and reuse it.
3. **Spectral libraries**: if the camera's spectral sensitivities are known
   (measured per phone model) and the panel technology is known (WLED, KSF/PFS,
   QD, WOLED, QD-OLED), compute the matrix the way colorimeters use `.ccss`
   spectral samples.

### Rules for camera capture

- RAW only. Processed JPEG/HEIC and browser `getUserMedia` frames have
  auto exposure, white balance, tone mapping and sharpening applied.
  Android: Camera2 `RAW_SENSOR`. iOS: Bayer RAW (not ProRAW, which is
  already processed).
- Lock exposure, ISO and focus. Defocus on purpose so the subpixel grid does
  not cause moiré.
- Exposure time must be a whole multiple of the refresh/PWM period, or OLED and
  PWM-dimmed panels will band and flicker in the measurement.
- Subtract the black level, average several frames, and keep every channel
  below roughly 80% of saturation to stay in the linear range of the sensor.
- Average the same central region for every patch. Lens vignetting then
  cancels out for tone and gray measurements; uniformity maps need a
  flat-field correction.

## Hardware sensors

Targets, in order of how common they are:

- Calibrite (ex X-Rite) Display Pro / Plus HL, i1Display Pro, ColorMunki
  Display, plus OEM-branded variants of the same hardware (USB HID).
- Datacolor SpyderX / SpyderX2 (USB).
- Spectrophotometers such as the i1Pro family, used as references and to
  create spectral corrections.

ArgyllCMS has the best open documentation of these protocols in the form of
working drivers. colibri is AGPLv3 like Argyll, so it reuses Argyll's
instrument layer (`spectro/` and what it depends on) directly: compiled as a
static C library and called from Rust over FFI. It does not drive the Argyll
command-line tools. On Windows those tools read keystrokes from the console
rather than from stdin, so controlling them from another program needs
console emulation, which is one source of DisplayCAL's fragility.

Argyll's CLI (`colprof`, `dispwin`, `profcheck`) is still useful as a
reference: tests compare colibri's profiles against what Argyll produces from
the same measurements.

## Architecture (proposed)

```
            +---------------------------- colibri desktop ----------------------------+
 sensor --->| instruments    trait Instrument { measure(patch) -> Reading }           |
 (USB)      |   argyll-spotread | i1d3 | spyder | camera-remote | simulated          |
            |                                                                         |
 phone ---->| camera-remote  WebSocket over LAN, paired by QR code; the phone sends  |
 (RAW)      |                linear RAW triplets for a fixed region of interest     |
            |                                                                         |
            | core           XYZ/xyY/Lab, ΔE2000, adaptation, matrix fitting, TRC     |
            |                fitting, 1D LUT generation, ICC v4 writer (+vcgt, MHC2)  |
            |                                                                         |
            | patch window   fullscreen patches, explicit bit depth, no OS colour     |
            |                management applied to the patches                       |
            |                                                                         |
            | os             load calibration curves, install profiles:              |
            |                macOS ColorSync, Windows ACM/MHC2,                       |
            |                Linux colord and Wayland color-management-v1             |
            +-------------------------------------------------------------------------+
```

Proposed stack: a Rust core (cross-platform, `hidapi`/`rusb` for sensors,
single static binary, which avoids DisplayCAL's Python packaging problems)
with a thin native or web UI on top. The phone companion is a native app,
because RAW capture is not available from a browser.

The `simulated` instrument comes first: it returns XYZ and RAW values
generated from spectra, as in `research/`, so the full pipeline can be tested
in CI without any hardware.

## Roadmap

1. **core + simulated instrument**: colour maths, profile fitting, ICC writer,
   with tests against the simulator.
2. **Argyll instrument layer over FFI**: real sensors, patch window, MHC2
   profile install on Windows 11.
3. **Phone camera mode**: gamma, gray balance and uniformity; EDID prior for
   absolute values.
4. **Rust drivers** to replace the C ones, only where they are clearly worth
   it.
5. **HDR**: PQ tone response, peak luminance and EOTF tracking checks.

## Decisions

- Licence: AGPLv3 (the full text still needs to be added as `LICENSE`).
- First OS: Windows 11. Profiles carry an `MHC2` tag so Windows' colour
  pipeline applies the calibration with high precision, including in HDR and
  Auto Color Management mode where the classic gamma ramp is not used.
- Instruments: Argyll's driver code linked in-process.

## Open decisions

- UI toolkit.
