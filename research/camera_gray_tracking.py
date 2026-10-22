"""
Can an uncharacterized camera calibrate a display?

Simulation with synthetic spectra. Two experiments:

  A. "Camera as colorimeter": convert camera RAW -> XYZ with a matrix fitted
     on a *different* display technology, then measure the white point of the
     target display. This is what naive phone-calibration apps do.

  B. "Camera as relative instrument": photograph full R, G, B patches to learn
     the camera->display-linear-RGB matrix for *this* display, then measure
     each channel's tone curve and build 1D calibration curves (gamma 2.2,
     neutral grays at native white). No camera characterization needed.

Ground truth is computed with (an analytic fit of) the CIE 1931 2-deg observer.

Run: python3 research/camera_gray_tracking.py
"""
import numpy as np

wl = np.arange(380.0, 781.0, 1.0)


def g(mu, s1, s2):
    s = np.where(wl < mu, s1, s2)
    return np.exp(-0.5 * ((wl - mu) / s) ** 2)


# CIE 1931 colour matching functions, multi-lobe fit (Wyman, Sloan, Shirley 2013)
CMF = np.stack([
    1.056 * g(599.8, 37.9, 31.0) + 0.362 * g(442.0, 16.0, 26.7) - 0.065 * g(501.1, 20.4, 26.2),
    0.821 * g(568.8, 46.9, 40.5) + 0.286 * g(530.9, 16.3, 31.1),
    1.217 * g(437.0, 11.8, 36.0) + 0.681 * g(459.0, 26.0, 13.8),
])

# A typical phone camera: smooth, overlapping sensitivities (violates the Luther condition)
CAM = np.stack([g(600, 35, 45), g(535, 40, 40), g(465, 25, 35)])


def display(peaks, widths, gains):
    return np.stack([k * g(p, w, w) for p, w, k in zip(peaks, widths, gains)])


# Target: a wide-gamut, narrow-primary panel (QD / OLED-like), slightly bluish native white
TARGET = display((630, 530, 455), (9, 13, 8), (1.00, 0.80, 0.95))
# Reference used to fit the naive camera matrix: a broad-primary sRGB-ish IPS/WLED panel
REFERENCE = display((610, 545, 450), (25, 30, 12), (1.0, 1.0, 1.0))

# Target's native tone response per channel: deliberately mismatched (uncalibrated)
NATIVE_GAMMA = np.array([2.05, 2.30, 2.55])


def xyz(spd):  return CMF @ spd
def raw(spd):  return CAM @ spd


def spd_for(drive, prims=TARGET, gamma=NATIVE_GAMMA):
    """Emitted spectrum for drive values in [0,1] (additive display model)."""
    return prims.T @ (np.asarray(drive) ** gamma)


def lab(XYZ, white):
    f = lambda t: np.where(t > (6 / 29) ** 3, np.cbrt(t), t / (3 * (6 / 29) ** 2) + 4 / 29)
    fx, fy, fz = f(XYZ / white)
    return np.array([116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)])


def de2000(lab1, lab2):
    L1, a1, b1 = lab1; L2, a2, b2 = lab2
    C1, C2 = np.hypot(a1, b1), np.hypot(a2, b2)
    Cm = (C1 + C2) / 2
    G = 0.5 * (1 - np.sqrt(Cm**7 / (Cm**7 + 25**7)))
    a1p, a2p = (1 + G) * a1, (1 + G) * a2
    C1p, C2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p = np.degrees(np.arctan2(b1, a1p)) % 360
    h2p = np.degrees(np.arctan2(b2, a2p)) % 360
    dLp, dCp = L2 - L1, C2p - C1p
    dh = h2p - h1p
    if C1p * C2p == 0: dh = 0
    elif dh > 180: dh -= 360
    elif dh < -180: dh += 360
    dHp = 2 * np.sqrt(C1p * C2p) * np.sin(np.radians(dh / 2))
    Lpm, Cpm = (L1 + L2) / 2, (C1p + C2p) / 2
    hs = h1p + h2p
    if C1p * C2p == 0: hpm = hs
    elif abs(h1p - h2p) <= 180: hpm = hs / 2
    else: hpm = (hs + 360) / 2 if hs < 360 else (hs - 360) / 2
    T = (1 - 0.17 * np.cos(np.radians(hpm - 30)) + 0.24 * np.cos(np.radians(2 * hpm))
         + 0.32 * np.cos(np.radians(3 * hpm + 6)) - 0.20 * np.cos(np.radians(4 * hpm - 63)))
    SL = 1 + 0.015 * (Lpm - 50) ** 2 / np.sqrt(20 + (Lpm - 50) ** 2)
    SC, SH = 1 + 0.045 * Cpm, 1 + 0.015 * Cpm * T
    RT = (-2 * np.sqrt(Cpm**7 / (Cpm**7 + 25**7))
          * np.sin(np.radians(60 * np.exp(-(((hpm - 275) / 25) ** 2)))))
    return np.sqrt((dLp / SL) ** 2 + (dCp / SC) ** 2 + (dHp / SH) ** 2
                   + RT * (dCp / SC) * (dHp / SH))


def xy(XYZ):
    return XYZ[:2] / XYZ.sum()


# ---------------------------------------------------------------- Experiment A
# Fit RAW->XYZ on the reference display (exact for *that* display: 3 primaries, 3x3)
M_naive = (CMF @ REFERENCE.T) @ np.linalg.inv(CAM @ REFERENCE.T)

white_true = xyz(spd_for([1, 1, 1]))
white_cam = M_naive @ raw(spd_for([1, 1, 1]))
white_cam *= white_true[1] / white_cam[1]  # compare chromaticity only

print("A. Camera as colorimeter (matrix fitted on another panel type)")
print(f"   true white    xy = {xy(white_true).round(4)}")
print(f"   camera white  xy = {xy(white_cam).round(4)}")
d65 = np.array([0.95047, 1.0, 1.08883]) * white_true[1]
print(f"   white error  dE00 = {de2000(lab(white_true, d65), lab(white_cam, d65)):.2f}"
      "   (a decent colorimeter: < 1)")
for name, d in (("red", [1, 0, 0]), ("green", [0, 1, 0]), ("blue", [0, 0, 1])):
    t, c = xyz(spd_for(d)), M_naive @ raw(spd_for(d))
    print(f"   {name:5s} primary  true xy {xy(t).round(3)}  camera xy {xy(c).round(3)}")

# ---------------------------------------------------------------- Experiment B
# Step 1: photograph full R, G, B. Columns of M_cam are their RAW triplets.
#         For an additive display, RAW = M_cam @ linear_rgb exactly.
rng = np.random.default_rng(1)
noise = lambda v: v * (1 + rng.normal(0, 0.002, v.shape))  # 0.2% shot/read noise
M_cam = np.column_stack([noise(raw(spd_for(e))) for e in np.eye(3)])
to_linear = np.linalg.inv(M_cam)

# Step 2: measure a 17-step ramp per channel through the camera only
steps = np.linspace(0, 1, 17)
trc = np.array([[(to_linear @ noise(raw(spd_for(np.eye(3)[c] * s))))[c] for s in steps]
                for c in range(3)])

# Step 3: 1D calibration curves (the VCGT): drive needed for linear = (input)^2.2.
#         Native white is kept (relative calibration), so each channel targets t * its max.
inp = np.linspace(0, 1, 256)
vcgt = np.array([np.interp(inp ** 2.2, trc[c], steps) for c in range(3)])

# Verify against ground-truth XYZ
print("\nB. Camera as relative instrument (no camera characterization)")
print("   level  gamma(meas)  dE00 vs ideal gray   | uncalibrated dE00")
Yw = white_true[1]
for i in (32, 64, 96, 128, 160, 192, 224):
    v = inp[i]
    ideal = white_true * v ** 2.2                      # white's chromaticity, Y = v^2.2
    got = xyz(spd_for(vcgt[:, i]))
    raw_uncal = xyz(spd_for([v, v, v]))
    gam = np.log(got[1] / Yw) / np.log(v)
    print(f"   {i:5d}  {gam:10.3f}   {de2000(lab(ideal, white_true), lab(got, white_true)):12.2f}"
          f"         | {de2000(lab(ideal, white_true), lab(raw_uncal, white_true)):10.2f}")
