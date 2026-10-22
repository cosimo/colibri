//! The GUI's flow, without egui: what the screens show and the names given
//! to profiles.

use std::path::PathBuf;
use std::time::Duration;

use colibri_windows::Monitor;

const SECONDS_PER_DAY: u64 = 86_400;

/// Whether the Ready screen offers a choice of display. With one display
/// there is nothing to choose.
pub fn show_display_choice(monitors: &[Monitor]) -> bool {
    monitors.len() > 1
}

/// Position of the primary display in `monitors`, or else the first.
pub fn default_monitor(monitors: &[Monitor]) -> usize {
    monitors.iter().position(|m| m.primary).unwrap_or(0)
}

/// How a display shows in the dropdown, such as
/// `1: ASUS PA328CGV, 2560×1440, primary`. The number tells two identical
/// monitors apart.
pub fn monitor_label(monitor: &Monitor) -> String {
    let number = monitor.index + 1;
    let (name, width, height) = (&monitor.name, monitor.width, monitor.height);
    let primary = if monitor.primary { ", primary" } else { "" };
    format!("{number}: {name}, {width}×{height}{primary}")
}

/// The progress line, such as
/// `Patch 23 of 44 · about 2 min left · Esc cancels`, once `done` of
/// `total` readings took `elapsed`. The time left is the average time per
/// reading so far, times the readings left, in whole minutes rounded up.
pub fn progress_text(done: usize, total: usize, elapsed: Duration) -> String {
    let patch = (done + 1).min(total);
    let can_estimate = done > 0 && done < total;
    if !can_estimate {
        return format!("Patch {patch} of {total} · Esc cancels");
    }

    let seconds_per_reading = elapsed.as_secs_f64() / done as f64;
    let seconds_left = seconds_per_reading * (total - done) as f64;
    let minutes_left = ((seconds_left / 60.0).ceil() as u64).max(1);
    format!("Patch {patch} of {total} · about {minutes_left} min left · Esc cancels")
}

/// The profile's file name, such as
/// `colibri-ASUS-PA328CGV-20261022-203251.icc`. Everything in the display
/// name but ASCII letters and digits becomes `-`, which keeps out the
/// characters Windows forbids in file names.
pub fn profile_file_name(display_name: &str, created: [u16; 6]) -> String {
    let safe_name: String = display_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let [year, month, day, hour, minute, second] = created;
    format!("colibri-{safe_name}-{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}.icc")
}

/// The profile's description, which is what Windows and the Compare screen
/// show, such as `colibri ASUS PA328CGV 2026-10-22 20:32 UTC`.
pub fn profile_description(display_name: &str, created: [u16; 6]) -> String {
    let [year, month, day, hour, minute, _] = created;
    format!("colibri {display_name} {year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Year, month, day, hour, minute and second in UTC of a Unix time. There
/// is no time zone crate, so profiles carry UTC times.
pub fn utc_from_unix(secs: u64) -> [u16; 6] {
    let (year, month, day) = civil_from_days(secs / SECONDS_PER_DAY);
    let seconds_today = secs % SECONDS_PER_DAY;
    let hour = seconds_today / 3600;
    let minute = seconds_today % 3600 / 60;
    let second = seconds_today % 60;
    [year, month, day, hour, minute, second].map(|v| v as u16)
}

/// The date `days` after 1970-01-01, with Howard Hinnant's algorithm. It
/// counts in 400-year eras that start on March 1st, so the leap day is the
/// last day of its year. See:
/// https://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    // Days from 0000-03-01 to 1970-01-01
    let days = days + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Months counted from March
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Where colibri keeps its profiles and restore file:
/// `%LOCALAPPDATA%\colibri`.
#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    let local_app_data = std::env::var_os("LOCALAPPDATA").unwrap_or_default();
    PathBuf::from(local_app_data).join("colibri")
}

/// Where colibri keeps its profiles and restore file:
/// `$XDG_DATA_HOME/colibri`, or else `~/.local/share/colibri`.
#[cfg(not(windows))]
pub fn data_dir() -> PathBuf {
    let data_home = std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty());
    if let Some(data_home) = data_home {
        return PathBuf::from(data_home).join("colibri");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join(".local/share/colibri")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(index: usize, name: &str, primary: bool) -> Monitor {
        Monitor {
            index,
            device: format!(r"\\.\DISPLAY{}", index + 1),
            name: name.into(),
            source: None,
            left: 0,
            top: 0,
            width: 2560,
            height: 1440,
            primary,
        }
    }

    #[test]
    fn display_choice_only_with_several_displays() {
        let asus = monitor(0, "ASUS PA328CGV", true);
        let dell = monitor(1, "DELL U2720Q", false);
        assert!(!show_display_choice(&[asus.clone()]));
        assert!(show_display_choice(&[asus, dell]));
    }

    #[test]
    fn default_monitor_is_the_primary() {
        let monitors = [monitor(0, "A", false), monitor(1, "B", true)];
        assert_eq!(default_monitor(&monitors), 1);
        assert_eq!(default_monitor(&[monitor(0, "A", false)]), 0);
    }

    #[test]
    fn monitor_label_names_the_display() {
        let label = monitor_label(&monitor(0, "ASUS PA328CGV", true));
        assert_eq!(label, "1: ASUS PA328CGV, 2560×1440, primary");
    }

    #[test]
    fn identical_displays_get_different_labels() {
        let first = monitor_label(&monitor(0, "ASUS PA328CGV", false));
        let second = monitor_label(&monitor(1, "ASUS PA328CGV", false));
        assert_ne!(first, second);
    }

    #[test]
    fn progress_text_estimates_time_left() {
        assert_eq!(
            progress_text(0, 44, Duration::ZERO),
            "Patch 1 of 44 · Esc cancels"
        );
        assert_eq!(
            progress_text(22, 44, Duration::from_secs(66)),
            "Patch 23 of 44 · about 2 min left · Esc cancels"
        );
        assert_eq!(
            progress_text(43, 44, Duration::from_secs(129)),
            "Patch 44 of 44 · about 1 min left · Esc cancels"
        );
    }

    #[test]
    fn progress_text_never_passes_the_total() {
        let text = progress_text(50, 44, Duration::from_secs(150));
        assert_eq!(text, "Patch 44 of 44 · Esc cancels");
    }

    #[test]
    fn profile_names_are_safe_file_names() {
        let created = [2026, 10, 22, 20, 32, 51];
        assert_eq!(
            profile_file_name("ASUS PA328CGV", created),
            "colibri-ASUS-PA328CGV-20261022-203251.icc"
        );
        let odd = profile_file_name(r#"a/b:c*?"<>|\"#, created);
        let forbidden = r#"/\:*?"<>|"#;
        assert!(!odd.contains(|c| forbidden.contains(c)), "{odd}");
    }

    #[test]
    fn profile_description_has_name_date_and_time() {
        let description = profile_description("ASUS PA328CGV", [2026, 10, 22, 20, 32, 51]);
        assert_eq!(description, "colibri ASUS PA328CGV 2026-10-22 20:32 UTC");
    }

    #[test]
    fn utc_from_unix_handles_epoch_and_leap_days() {
        assert_eq!(utc_from_unix(0), [1970, 1, 1, 0, 0, 0]);
        assert_eq!(utc_from_unix(951_782_400), [2000, 2, 29, 0, 0, 0]);
        assert_eq!(utc_from_unix(1_792_701_171), [2026, 10, 22, 20, 32, 51]);
    }
}
