# colibri GUI

Last updated: 2026-10-22 Cosimo

A desktop window for colibri, built with egui. The flow is the one the README
promises: pick the display, plug in the sensor, press Calibrate, get a
profile. Before keeping the new profile, I want to flip the whole display
between the old profile and the new one, and judge the difference by eye.

The first version runs on Windows 11 with my Spyder4. colibri can't drive
the Spyder4 itself yet, so for now it runs Argyll's `spotread` once per
patch. That is only for testing. Proper sensor support is a separate project
that comes after this one, and it plugs in behind the same `Instrument`
trait without changing the GUI.

## Goals

- One window, three screens: Ready, Measuring, Compare.
- The display choice is skipped when only one display is connected.
- Calibrate with the Spyder4 through `spotread`, or with the simulated sensor.
- After calibrating, a Before/After switch flips the whole display between
  the previous profile and the new one. No measurement is involved.
- Keep makes the new profile the display's default. Discard puts the
  previous one back.

## Non-goals for this version

- Measuring the display again after calibrating, or any verification report.
- Settings for the target. It is fixed at D65 white and the sRGB tone curve,
  the same as `colibri simulate`.
- A proper sensor driver. `spotread` is a test stand-in.
- macOS and Linux display integration. The GUI builds and runs elsewhere,
  but only with the simulated sensor, and the switch is disabled.
- HDR calibration.

## Anti-goals

These are my calls, from what a calibration tool must never do:

- Leave the display in a changed state after a cancel, an error or a crash:
  the profile removed, the gamma ramp not restored, or the new profile
  active without anyone choosing Keep.
- A console window flashing up for every `spotread` reading.
- A settings screen before the first calibration.

## The flow

### Ready

- Display: a dropdown of `colibri_windows::monitors()`, with the primary
  display selected. With one display it is plain text. Each display shows a
  friendly name, such as `ASUS PA328CGV`, its resolution, and `primary`.
- Sensor: a dropdown with "Spyder4 (through spotread, test)" and
  "Simulated". Next to it, whether a sensor was found. When `spotread` or
  the Spyder4 can't be found, the line says which, and Calibrate is
  disabled for that choice.
- Target: "D65 white, sRGB curve", as plain text.
- A short reminder: Night light and HDR off, monitor warmed up for about 30
  minutes.
- A large Calibrate button.

### Measuring

With the Spyder4, colibri's patch window opens full screen on the chosen
display:

1. It shows a white square and the text "Put the sensor on the square and
   press Space". Esc cancels.
2. After Space, it runs through the patches. With the default `Options`
   that is 44 readings: black, red, green and blue, 8 noise probe readings,
   and 32 grays.
3. Progress is a line of small text in a corner of the patch window, far
   from the square: "Patch 23 of 44 · about 1 min left · Esc cancels". With
   one display the patch window covers the colibri window, so progress
   can't live only there. With two displays, the colibri window shows it
   too.

With the simulated sensor there is no patch window, and the real display
is left alone while measuring. The simulator measures a simulated panel,
and progress shows in the colibri window.

Before the first patch, colibri takes the display out of colour management:
it records the current default profile, removes it as the default, and
resets the gamma ramp to identity. Afterwards, whatever happens, it puts
them back.

### Compare and keep

When measuring ends, colibri builds the profile, saves it, installs it and
makes it the display's default, so the screen opens with After on.

- A Before/After switch, with After selected. Space flips it as well.
- Before names the previous profile, or says "no profile". After names the
  new one. When the sensor was the simulator, After says so: it shows a
  made-up correction on the real display.
- A gray ramp and a few colour swatches, as something to look at. The
  switch changes the whole display, so any window on it can be compared.
- Keep new profile: the new profile stays the default, and the window shows
  where the file was saved.
- Discard: the previous profile becomes the default again.
- Closing the window without choosing counts as Discard.

### Errors and cancel

Esc in the patch window, closing the colibri window, or an error during
measuring all stop the run, restore the display, and return to Ready with
a message. The message says what failed in plain words, and for `spotread`
it includes the last lines of its output.

## Sensors

### Spyder4 through spotread (test only)

A new `Instrument` implementation, `SpotreadInstrument`, runs one
`spotread` process per reading:

    spotread -e -O -x <extra args>

`-O` reads straight away without waiting for a key, and exits after one
reading (`spectro/spotread.c`, around line 1887). The result line is

    Result is XYZ: 95.123 100.456 108.789, Yxy: ...

and `-e` makes it absolute XYZ in cd/m², so `trust()` is `Trust::Absolute`.

- The path to `spotread.exe` comes from `COLIBRI_SPOTREAD`, or else from
  `PATH`.
- The extra arguments come from `COLIBRI_SPOTREAD_ARGS`, default `-y e`
  (LCD, White LED Backlight). The display type matters a lot for the
  Spyder4: on a MacBook M4 screen, `-y B` read 0.005 further from D65 in x
  than `-y e`.
- "Sensor found" on the Ready screen comes from running `spotread -?` and
  looking for `Spyder` in its instrument list.
- Each process starts with `CREATE_NO_WINDOW`, so no console flashes up.
- A reading that takes longer than 60 seconds is an error. The Spyder4 can
  take several seconds on dark patches.
- Each reading reopens the sensor. I expect a second or two of extra time
  per reading (**? not measured yet**), so a calibration could take two or
  three minutes.

It pairs with `PatchWindow` through the existing `InstrumentRig`.

### Simulated

`SimRig::new(SimDisplay::wide_gamut(), SimSensor::colorimeter(1))`, the same
rig `colibri simulate` uses.

## Profiles on Windows

### Building the profile

`colibri simulate` builds its `DisplayProfile` inline in
`crates/colibri-cli/src/main.rs`. That code moves into one function in
`colibri-core`, which both the CLI and the GUI call. The profile carries
the calibration curves (`vcgt`) and an `MHC2` tag.

The `MHC2` tag uses `Mhc2Gamut::Native`: only the calibration curves apply,
and colours are left alone. The display I'm calibrating is an ASUS ProArt
PA328CGV, a wide-gamut panel (95% DCI-P3) used for photo editing. Photo
editors map colours through the profile and need the full gamut. Everything
else gets mapped by Windows' Auto Color Management, which reads the
profile's primaries. `colibri simulate` keeps `Emulate` sRGB.

The description is `colibri <display name> <date> <time>`, which is what
Before and After show. The file is saved in `%LOCALAPPDATA%\colibri\profiles\`
before it is installed.

### Switching between Before and After

Both directions change which profile is the display's default, so Windows
applies its `vcgt` and `MHC2` the way it would after a reboot. This is the
same mechanism Keep and Discard use, so what the switch shows is what Keep
leaves behind.

- Profiles are associated per user, so no administrator prompt is needed
  (**? to be validated: `InstallColorProfileW` may need admin rights**).
- Windows 11 has `ColorProfileAddDisplayAssociation`,
  `ColorProfileSetDisplayDefaultAssociation` and `ColorProfileGetDisplayDefault`
  in `mscms`. They identify a display by adapter LUID and source id from
  `QueryDisplayConfig`, not by the GDI name `\\.\DISPLAY1` that `Monitor`
  holds. The same lookup gives the friendly name for the Ready screen.
- I don't know how long a switch takes, or whether it flickers
  (**? to be validated on real hardware**).
- If Windows doesn't apply an `MHC2` profile through these calls without a
  re-login, the fallback is loading the new curves into the gamma ramp with
  `set_gamma_ramp`, which is instant but leaves out the `MHC2` matrix.

### Restoring after a crash

Before changing anything, colibri writes the display's previous default
profile to `%LOCALAPPDATA%\colibri\restore.json`, and deletes it once the
display is back to a chosen state. If the file is there at startup, colibri
restores that profile first and says so.

### What the user must still do by hand

Auto Color Management and HDR have to be off while measuring. If the GUI
can detect them through `DisplayConfigGetDeviceInfo`, it refuses to start
and says which setting to turn off, for which display. Otherwise the Ready
screen reminder covers it. colibri doesn't change these settings itself.

With a `Native` profile, Auto Color Management is what keeps ordinary apps
from looking oversaturated, so day to day it stays on. That means turning it
off for each calibration, and back on afterwards. The Compare screen
reminds the user to turn it back on (we could maybe automate this later, if
Windows offers a call for it).

## Code layout

- `crates/colibri-gui`: the egui app, built with `eframe`, binary
  `colibri-gui`. In release builds on Windows it uses the GUI subsystem, so
  no console opens with it.
- `crates/colibri-spotread`: `SpotreadInstrument` and the parser for
  `spotread` output. It sits in its own crate so it can be deleted when the
  real driver lands, and the CLI could use it too.
- `crates/colibri-windows`: display identity and friendly names from
  `QueryDisplayConfig`, profile install and default association, and two
  patch window additions: wait for Space with a prompt, and a status text
  line in a corner. Stubs on other systems return `Error::Unsupported`, as
  the rest of the crate does.
- `crates/colibri-core`: the shared profile builder, and a function that
  returns how many readings `calibrate()` takes for given `Options`, so the
  progress total can't drift from the real count.

The egui layer stays thin. The flow lives in plain functions and a small
state type in their own module, which know nothing about egui: what the
Ready screen offers, starting a run, handling progress, Keep, Discard,
restore. Calibration runs on a worker thread and sends progress and the
result back over a channel. The UI asks for a repaint on each message.

## Testing

- The `spotread` output parser, against real output captured from the
  Spyder4, including an error run.
- The reading count function, against a rig that counts `measure()` calls,
  for a few `Options` combinations.
- The flow state, with the simulated rig and a fake profile store: cancel
  and error restore the previous profile, closing during Compare discards,
  a leftover restore file is applied at startup.
- The Windows profile calls and the patch window additions can only be
  tested by hand on Windows. A short section in
  `docs/windows-spyder4-setup.md` covers it.

## Decided by Cosimo

- egui.
- The three-screen flow, the display choice only with more than one display.
- Before/After by switching profiles on the whole display, without measuring.
- `spotread` once per patch is acceptable for testing, not for the final
  build.

## My calls

- Target fixed at D65 and the sRGB curve.
- `Mhc2Gamut::Native` for the GUI's profiles, for photo editing on the
  PA328CGV.
- The simulated sensor selectable in the GUI.
- Space to start measuring and to flip Before/After. Closing without
  choosing means Discard.
- Configuring `spotread` through `COLIBRI_SPOTREAD` and
  `COLIBRI_SPOTREAD_ARGS`, with `-y e` as the default.
- Per-user profile association, the restore file, the 60 second timeout.
- The crate split, and the separate `colibri-gui` binary next to `colibri`.

## Left to the builder

- The `eframe` version. Pin it exactly, as `windows-sys` is. It may need a
  newer `rust-version` than the workspace's 1.82.
- The look of the screens, within the flow described above.
- How the patch window draws its status text.
- The settle time for `InstrumentRig` with `spotread`.
