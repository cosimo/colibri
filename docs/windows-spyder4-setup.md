# Testing colibri on Windows 11 with a Spyder4

This guide covers getting a Datacolor Spyder4 (Express, Pro or Elite: same
hardware) reading through ArgyllCMS, checking how accurate it still is, and
testing colibri's patch window against it. colibri can't drive the sensor
itself yet, so it runs Argyll's `spotread` once for each reading.

Commands that read the instrument come from Argyll. Their options vary
between versions, so check `spotread -?` and the
[Argyll documentation](https://www.argyllcms.com/doc/ArgyllDoc.html) whenever
something below doesn't match what you see.

## 1. Install ArgyllCMS and its USB driver

1. Download the Windows 64-bit build from
   [argyllcms.com](https://www.argyllcms.com/) and unzip it somewhere
   permanent, for example `C:\Argyll`.
2. Add its `bin` folder (`C:\Argyll\bin`) to your `PATH`.
3. Install Argyll's USB driver for the Spyder4. Argyll talks to the sensor
   through its own libusb-based driver, which replaces Datacolor's for that
   device. The two can't both use it at once. Follow
   [Installing on Microsoft Windows](https://www.argyllcms.com/doc/Installing_MSWindows.html),
   or use DisplayCAL's "Install ArgyllCMS instrument drivers" if you have it.
4. Plug the Spyder4 in and open Device Manager. It should appear under the
   Argyll driver entry, not under Datacolor's. If it still shows Datacolor's
   driver, Argyll won't find it: this is the most common reason for
   "Spyder4 not detected".

## 2. Install the Spyder4 calibration data

Without Datacolor's calibration data, the Spyder4 only offers a generic
display type and will be off on most modern panels. Argyll's `oeminst`
extracts the data (`spyd4cal.bin`) from a Datacolor installer and also
installs spectral correction files
([oeminst docs](https://argyllcms.com/doc/oeminst.html)).

```
oeminst
```

With no arguments it looks for installed or inserted Datacolor software.
Otherwise, pass the path of a Datacolor installer: a newer one such as a
Spyder5 installer also works. It reports what it installed and where.

## 3. Check the sensor reads

```
spotread -?
```

The Spyder4 should be listed among the instruments, and the `-y` option
should now list several display types (LCD, wide gamut, LED and so on).
Note the letter for the type that matches your monitor.

Take a first reading of a white screen:

```
spotread -x -y <letter>
```

`-x` prints Yxy (luminance and chromaticity) instead of Lab. Put the sensor
flat on the screen, press a key to read, and `q` to quit.

## 4. Check how accurate the sensor still is

The Spyder4's dye filters may have drifted over 13 years. To check, measure
the white of a screen that is factory-calibrated close to D65, such as a
recent iPhone or MacBook:

1. Set that screen to full brightness, no Night Shift or True Tone, and show
   a plain white image.
2. Run `spotread -x -y <letter>` with the display type closest to that
   screen (an OLED type for an iPhone, if one is listed).
3. Compare x and y with D65: **x 0.3127, y 0.3290**.

This is a rough check: those screens have their own tolerance of a few
thousandths, and the display-type correction matters. As a guide, results
within about 0.005 of D65 in both x and y mean the sensor is probably fine
for absolute targets. Results further off mean colibri should treat it as a
relative instrument (gamma and gray balance only).

## 5. Build colibri

Install Rust with [rustup](https://rustup.rs) (default options: it will also
offer to install the Visual Studio build tools), then:

```
git clone -b claude/monitor-color-calibration-dip328 https://github.com/cosimo/colibri
cd colibri
cargo run --release -p colibri-cli -- simulate
```

`simulate` runs a calibration against the built-in simulator and prints the
result. It confirms the build works.

## 6. Prepare the display to be measured

Windows must show the patches untouched:

1. **Auto Color Management off:** Settings → System → Display → choose the
   monitor → turn off "Automatically manage color for apps" if it is there.
2. **No calibration profile active:** run `colorcpl`, select the monitor on
   the Devices tab, and remove any calibration profile, especially one with
   an MHC2 tag or from a vendor calibration tool. Note what was there so you
   can restore it.
3. **Night light and HDR off** for that monitor.
4. **Reset the GPU gamma ramp**, then check it:

   ```
   cargo run --release -p colibri-cli -- displays
   cargo run --release -p colibri-cli -- gamma reset --monitor 0
   cargo run --release -p colibri-cli -- gamma show --monitor 0
   ```

   `gamma show` should end with `identity: true`. Use the monitor number from
   `displays`.

5. Let the monitor warm up for about 30 minutes, at the brightness and
   settings you normally use.

## 7. Test colibri's patch window

`colibri check` opens the patch window, reads seven patches through
`spotread` and prints the readings. Nothing needs typing during the run.

First tell it the display type letter from step 3, if it isn't `e`. In
PowerShell:

```
$env:COLIBRI_SPOTREAD_ARGS = "-y e"
```

or in cmd, `set COLIBRI_SPOTREAD_ARGS=-y e`. If `spotread` isn't on `PATH`,
set `COLIBRI_SPOTREAD` to the full path of `spotread.exe`. Then:

```
cargo run --release -p colibri-cli -- check --monitor 0
```

It looks for the Spyder in `spotread -?` first. Then a black screen with a
white square in the middle covers the monitor. Put the sensor flat in the
middle of the square, and press Space or click anywhere on the black. The
square goes through white, black, gray, red, green, blue and white again,
one reading each, with the progress in the bottom-left corner. Esc cancels.
Each reading takes a few seconds, since `spotread` opens the sensor every
time.

The window closes at the end, and the terminal shows something like this
(made-up numbers):

```
patch    R   G   B   Y cd/m² of white       x       y
white  255 255 255   120.312   100.0%  0.3135  0.3301
black    0   0   0     0.214     0.2%  0.2690  0.2810
gray   128 128 128    25.871    21.5%  0.3131  0.3298
red    255   0   0    26.402    21.9%  0.6402  0.3301
green    0 255   0    80.113    66.6%  0.3005  0.6003
blue     0   0 255    13.987    11.6%  0.1503  0.0601
white  255 255 255   120.104    99.8%  0.3134  0.3300
additivity error 0.31%: red + green + blue against white
white drift: Y -0.17%, xy 0.0001
```

What a working setup shows:

- Black: a very low Y, around 0.1–0.5 cd/m² on a typical LCD.
- Gray: Y roughly 20% of white, with x, y close to white's.
- Red: x well above 0.6. Green: y well above 0.6. Blue: x around 0.15,
  y below 0.1.
- Additivity error: within a few percent. A much bigger one means the
  panel's channels interact, or Windows is still transforming the patches
  (go back to step 6).
- White drift: within about 1% in Y. If it drifts more, give the monitor
  longer to warm up.

`check` warns when the monitor has a gamma ramp loaded. Its readings are
then not the panel's own, so reset the ramp as in step 6 and run it again.

`colibri patch --monitor 0 R G B` still shows a single patch, for spot
checks with `spotread` by hand. It needs a second monitor for the
terminals, since the patch window covers the one it is on.

## 8. Try the GUI

`colibri-gui` runs the whole calibration: it measures 44 patches, builds
the profile, installs it, and lets you flip the display between the
previous profile and the new one before keeping it.

It reads the Spyder4 the same way `colibri check` does, so the
`COLIBRI_SPOTREAD_ARGS` setting from step 7 applies. If Argyll isn't on
`PATH`, set `COLIBRI_SPOTREAD` to the full path of `spotread.exe`. In
PowerShell:

```
$env:COLIBRI_SPOTREAD = "C:\Argyll\bin\spotread.exe"
cargo run --release -p colibri-gui
```

A release build opens no console window. `cargo run -p colibri-gui`, a
debug build, keeps one, which helps if something goes wrong.

Auto Color Management, Night light and HDR still have to be off, as in
step 6. The GUI removes the display's profile and resets the gamma ramp by
itself before measuring, and puts both back afterwards. Profiles are saved
in `%LOCALAPPDATA%\colibri\profiles\`.

Please try these, and note what happened for each:

- The Ready screen with one display, and with two: the display names, and
  whether the one chosen is the primary.
- Auto Color Management left on: whether Calibrate is disabled, with a
  message naming the setting and the display. If Calibrate stays enabled,
  colibri couldn't read the colour mode.
- A full Spyder4 run: how long it took, and whether the progress line in
  the corner ever showed through the patch.
- Esc during measuring: whether the previous profile is back in `colorcpl`,
  and whether `colibri gamma show` says `identity: true` only if it did
  before the run.
- The Before/After switch, with the mouse or Space: how long each flip
  takes, any flicker, and whether the new profile visibly applies without
  signing out. If it doesn't, colibri needs another way to switch.
- Whether installing the profile asked for administrator rights.
- Keep, and in another run Discard: which profiles `colorcpl` then shows
  for the display, and which one is the default.
- Ending colibri from Task Manager while in Compare, then starting it
  again: the message on the Ready screen, and whether the previous profile
  is the default again.
- The output of these two, pasted as they are:

  ```
  spotread -e -O -x -y e
  spotread -?
  ```

  The first takes one reading straight away, so put the sensor on
  something lit. These replace the made-up samples in
  `crates/colibri-spotread/src/lib.rs` with real ones.

## 9. What to report back

Please paste:

1. The output of `spotread -?` (at least the instrument list and the `-y`
   display types).
2. The display type you chose, and the D65 check from step 4 with the
   reference screen used.
3. The output of `colibri check` from step 7.
4. The output of `colibri displays` and `colibri gamma show`.
5. Anything that misbehaved: the patch window not appearing or not covering
   the monitor, wrong colours, Space or the click not starting the run, a
   reading timing out, or error messages.
6. What you noted for each item in step 8.

From that, colibri's Spyder4 backend gets its trust level and display-type
correction, and the patch window gets its first real test.
