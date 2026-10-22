# Windows 11 display calibration landscape (October 2026)

Survey of what exists, to answer one question: is there room for colibri?
Sources are listed at the end. Items marked *(unverified)* are things I could not
confirm from a primary source.

## How Windows 11 applies a calibration now

The details below are what decides where a new tool can do better.

- Windows 10 2004 and later have a GPU colour pipeline that an ICC profile
  drives through a private `MHC2` tag. It is more precise than the old GDI
  gamma ramp and also works in HDR. The pipeline is a fixed degamma, then a
  3x3 matrix, then per-channel 1D "regamma" LUTs. [MS Learn, MS DevBlogs]
- Windows 11 24H2 with a valid MHC profile uses only `lumi`, `MHC2` and the
  primaries, and ignores the profile's tone curves and `vcgt`. [dantmnf/MHC2]
  So a classic DisplayCAL/Argyll profile does not calibrate anything in that
  mode until it is converted.
- The Windows calibration loader has to be enabled for MHC2 to load.
  DisplayCAL's own profile loader disables that scheduled task.
  [dantmnf/MHC2]
- Because the degamma is fixed, results are exact only when the target tone
  curve is sRGB-like. [novideo_srgb author]
- Reported driver problems with MHC active: cursor/MPO glitches on NVIDIA and
  AMD, and poor LUT quality on Intel with 8-bit output. [dantmnf/MHC2]
- SDR and HDR use separate profiles. Much of the behaviour is documented only
  through reverse engineering and changes between Windows releases.

## Tools

### Vendor software (locked to one sensor brand)

| Tool | State | Notes |
|---|---|---|
| Calibrite PROFILER | 3.1.0, May 2026 | Calibrite and X-Rite sensors only. Windows 11 users report a "won't launch" bug that a later build fixed. |
| Datacolor Spyder apps | SpyderX2 Windows 6.0.1, last change 2023 | Features depend on which product tier was bought. Activation now needs an internet connection. Upgrade-licence problems in 2024. |
| Calman (Portrait Displays) | 5.17, March 2026 | Professional, aimed at TVs and studios, expensive. "Calman Home" (2019, $145) was specific to TV brands. |

I found no sign that any of these write MHC2 profiles or support Windows
Auto Color Management *(unverified)*.

### Open source

| Tool | State | Notes |
|---|---|---|
| ArgyllCMS | 3.4.x to 3.5, active | The reference engine. Supports old and new sensors (SpyderX2 and Spyder 2024 added recently). Command line only. Does not produce MHC2. |
| DisplayCAL (py3 fork) | 3.9.19, July 2026, active again | wxPython, with a Qt port still experimental. Release notes do not mention MHC2 or ACM. Its developer once said they had no HDR or Windows 11 test machine. |
| MHC2Gen (dantmnf) | experimental | Converts a DisplayCAL profile to an MHC2 profile. Tested with DisplayCAL profiles only. |
| MHC ICC Profile Maker | small, about a year since last update | GUI for editing MHC2 profiles, computes a matrix from W/R/G/B readings. |
| novideo_srgb / msovideo_srgb | active forks | Clamps a wide-gamut panel to sRGB through GPU driver APIs or MHC2. |
| dwm_lut and forks | fragile | Injects a 3D LUT into the Windows compositor. Breaks with Windows updates; whether it works on 24H2/25H2 depends on the fork. |
| Windows HDR Calibration, Windows DCC | built in | Adjusted by eye. No measurement. |

### Phone and camera based

| Tool | Notes |
|---|---|
| TruHu | iPhone plus Windows/macOS, $39.99 per year. Reviewed as easy to use and aimed at consumers. No published accuracy figures against a colorimeter. |
| Microsoft "Display Calibration" + Android "Camera Colorimeter" | About 10 years old, sRGB only. |
| Samsung Smart Calibration | Phone camera calibrates Samsung TVs and Smart Monitors through SmartThings. |
| Apple TV Color Balance | Uses the iPhone's sensors to correct the Apple TV's video output. |

Big consumer brands already ship phone-camera calibration for TVs, so people
accept the idea. None of the PC tools I found publish accuracy data.

## Where the gaps are

1. **Nothing does the whole job on Windows 11.** The current best free route
   takes several tools: measure with Argyll, profile with DisplayCAL, convert
   with MHC2Gen, re-enable the Windows loader, then hope the driver
   cooperates. Doing all of that in one tool, for SDR and HDR, is the clearest
   opportunity.
2. **Checking that the calibration is actually applied.** Forums are full of
   "the profile is loaded but nothing changed". A tool that measures the
   screen after installing, through the real pipeline, and names the cause
   (loader disabled, ACM off, conflicting tool, driver bug) would solve a
   real, common problem.
3. **Any sensor, including old ones.** Vendor apps support one brand and
   gate features by product tier. Argyll supports nearly everything, but only
   through DisplayCAL's interface.
4. **Honest phone mode.** Existing phone apps imply absolute accuracy without
   publishing data. Phone mode can calibrate gamma and gray balance exactly
   (see `research/`), measure uniformity, and say clearly what it cannot
   know.

## Risks

- DisplayCAL is maintained again and could add MHC2 support. MHC2Gen already
  shows it is possible.
- Microsoft changes the MHC behaviour between releases without documenting it.
- GPU driver bugs in the MHC path are outside colibri's control.
- The colour science (Argyll) is already excellent, so colibri's value is
  integration, Windows expertise and UX, not better maths. The exception is
  phone mode.

## Sources

- Microsoft Learn, display calibration with MHC: https://learn.microsoft.com/en-us/windows/win32/wcs/display-calibration-mhc
- Microsoft DevBlogs, Auto Color Management (Oct 2022): https://devblogs.microsoft.com/directx/auto-color-management
- dantmnf/MHC2 (reverse-engineered MHC behaviour, MHC2Gen): https://github.com/dantmnf/MHC2
- MHC ICC Profile Maker: https://github.com/ttys001/MHC-ICC-Profile-Maker
- msovideo_srgb: https://github.com/guzenco/msovideo_srgb
- novideo_srgb thread (MHC2 limits): https://hub.displaycal.net/forums/topic/novideo_srgb-gpu-side-lut-matrix-lut-calibration/page/11/
- DisplayCAL py3 releases: https://github.com/eoyilmaz/displaycal-py3/releases
- DisplayCAL on PyPI: https://pypi.org/project/displaycal/
- DisplayCAL forum, Windows ACM: https://hub.displaycal.net/forums/topic/windows-auto-color-management-acm/
- DisplayCAL forum, HDR ICC profiles: https://hub.displaycal.net/forums/topic/are-there-real-hdd-icc-display-profiles/
- ArgyllCMS changes: https://argyllcms.com/doc/ChangesSummary.html
- dwm_lut: https://www.Github.com/ledoge/dwm_lut and https://hub.displaycal.net/forums/topic/windows-24h2-dwmlut-srgb-vs-gamma-2-2/
- Calibrite PROFILER release notes: https://calibrite.com/learning-centre/calibrite-profiler-release-notes/
- Calibrite downloads: https://calibrite.com/software-downloads
- Calibrite launch bug thread: https://www.fredmiranda.com/forum/topic/1918967/
- Datacolor SpyderX2 Windows: https://spyder-support.datacolor.com/hc/en-us/articles/8755167675676
- Datacolor Spyder X Elite Windows: https://spyder-support.datacolor.com/hc/en-us/articles/4403402987154-Spyder-X-Elite-5-9-Windows
- Calman Home pricing: https://www.portrait.com/calman-home-lowering-the-price-of-home-calibration-software/
- Portrait Displays downloads: https://portrait.com/software-downloads
- TruHu: https://truhu.app/ and https://petapixel.com/2023/06/06/truhu-review-a-simple-way-to-calibrate-monitors-with-a-smartphone/
- Microsoft Display Calibration app: https://apps.microsoft.com/detail/9nblggh4wd9s
- Samsung Smart Calibration: https://www.samsung.com/uk/support/apps-services/how-to-calibrate-your-tv-screen-with-smartthings/
- Apple TV Color Balance: https://reviewed.com/home-theater/features/how-use-apple-tv-color-calibration-iphone-feature
