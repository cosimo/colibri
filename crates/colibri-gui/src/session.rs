//! Switching a display between its previous profile and the new one, and
//! putting it back after a cancel, an error or a crash.
//!
//! Before anything changes, the display's default profile goes into a
//! restore file. It is removed once the display is back in a state someone
//! chose: Keep, Discard, or the previous profile after a cancel. If colibri
//! dies in between, the next start finds the file and puts the profile back.

use std::io;
use std::path::{Path, PathBuf};

use colibri_windows::{identity_ramp, Error, GammaRamp, Monitor};
use serde::{Deserialize, Serialize};

/// The profile and gamma ramp calls a session makes, so tests can run them
/// against a store in memory.
pub trait ProfileStore {
    fn default_profile(&mut self, monitor: &Monitor) -> Result<Option<String>, Error>;
    fn associate(
        &mut self,
        monitor: &Monitor,
        profile: &str,
        make_default: bool,
    ) -> Result<(), Error>;
    fn dissociate(&mut self, monitor: &Monitor, profile: &str) -> Result<(), Error>;
    fn install(&mut self, path: &Path) -> Result<String, Error>;
    fn gamma_ramp(&mut self, monitor: &Monitor) -> Result<GammaRamp, Error>;
    fn set_gamma_ramp(&mut self, monitor: &Monitor, ramp: &GammaRamp) -> Result<(), Error>;
}

/// Calls straight through to `colibri_windows`.
pub struct WindowsProfiles;

impl ProfileStore for WindowsProfiles {
    fn default_profile(&mut self, monitor: &Monitor) -> Result<Option<String>, Error> {
        colibri_windows::default_profile(monitor)
    }

    fn associate(
        &mut self,
        monitor: &Monitor,
        profile: &str,
        make_default: bool,
    ) -> Result<(), Error> {
        colibri_windows::associate_profile(monitor, profile, make_default)
    }

    fn dissociate(&mut self, monitor: &Monitor, profile: &str) -> Result<(), Error> {
        colibri_windows::dissociate_profile(monitor, profile)
    }

    fn install(&mut self, path: &Path) -> Result<String, Error> {
        colibri_windows::install_profile(path)
    }

    fn gamma_ramp(&mut self, monitor: &Monitor) -> Result<GammaRamp, Error> {
        colibri_windows::gamma_ramp(monitor)
    }

    fn set_gamma_ramp(&mut self, monitor: &Monitor, ramp: &GammaRamp) -> Result<(), Error> {
        colibri_windows::set_gamma_ramp(monitor, ramp)
    }
}

#[derive(Debug)]
pub enum SessionError {
    Display(Error),
    /// The restore file couldn't be written or removed.
    RestoreFile(PathBuf, io::Error),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Display(e) => e.fmt(f),
            SessionError::RestoreFile(path, e) => write!(f, "{}: {e}", path.display()),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<Error> for SessionError {
    fn from(e: Error) -> SessionError {
        SessionError::Display(e)
    }
}

/// What the restore file holds: the display and the default profile it had
/// before colibri changed anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RestoreRecord {
    pub device: String,
    pub profile: Option<String>,
}

pub struct RestoreFile {
    pub path: PathBuf,
}

impl RestoreFile {
    /// Writes the record, creating the directory if needed.
    pub fn save(&self, record: &RestoreRecord) -> Result<(), SessionError> {
        let failed = |e: io::Error| SessionError::RestoreFile(self.path.clone(), e);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let json = serde_json::to_string_pretty(record).map_err(|e| failed(e.into()))?;
        std::fs::write(&self.path, json).map_err(failed)
    }

    /// The record, if there is a readable one.
    pub fn load(&self) -> Option<RestoreRecord> {
        let json = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Removes the file. A file that is already gone is fine.
    pub fn clear(&self) -> Result<(), SessionError> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                Err(SessionError::RestoreFile(self.path.clone(), e))
            }
            _ => Ok(()),
        }
    }
}

/// Which profile the display is showing in Compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Showing {
    Before,
    After,
}

/// One display's profile changes, from the start of a run to Keep or
/// Discard.
pub struct DisplaySession {
    pub monitor: Monitor,
    /// The default profile before the run.
    pub before: Option<String>,
    /// The new profile, once installed.
    pub new_profile: Option<String>,
    pub showing: Showing,
    /// The gamma ramp found at `begin()`.
    original_ramp: GammaRamp,
}

impl DisplaySession {
    /// Records the display's state in the restore file. With
    /// `prepare_for_measuring`, also dissociates `before` and resets the
    /// gamma ramp to identity, so the patches reach the panel untouched.
    pub fn begin(
        store: &mut dyn ProfileStore,
        restore: &RestoreFile,
        monitor: Monitor,
        prepare_for_measuring: bool,
    ) -> Result<DisplaySession, SessionError> {
        let before = store.default_profile(&monitor)?;
        let original_ramp = store.gamma_ramp(&monitor)?;
        let record = RestoreRecord {
            device: monitor.device.clone(),
            profile: before.clone(),
        };
        restore.save(&record)?;

        let session = DisplaySession {
            monitor,
            before,
            new_profile: None,
            showing: Showing::Before,
            original_ramp,
        };
        if !prepare_for_measuring {
            return Ok(session);
        }

        let Err(e) = session.prepare_for_measuring(store) else {
            return Ok(session);
        };
        // Put back whatever changed before the failure. If that fails too,
        // the restore file stays for the next start.
        let _ = session.restore(store, restore);
        Err(e.into())
    }

    /// Takes the display out of colour management: no default profile, and
    /// an identity gamma ramp.
    fn prepare_for_measuring(&self, store: &mut dyn ProfileStore) -> Result<(), Error> {
        if let Some(before) = &self.before {
            store.dissociate(&self.monitor, before)?;
        }
        store.set_gamma_ramp(&self.monitor, &identity_ramp())
    }

    /// Installs the new profile and makes it the default, so the display
    /// shows After.
    pub fn install_new(
        &mut self,
        store: &mut dyn ProfileStore,
        path: &Path,
    ) -> Result<(), SessionError> {
        let name = store.install(path)?;
        store.associate(&self.monitor, &name, true)?;
        self.new_profile = Some(name);
        self.showing = Showing::After;
        Ok(())
    }

    /// Makes the display show the previous profile, or the new one. Before
    /// with no previous profile means no profile at all.
    pub fn show(
        &mut self,
        store: &mut dyn ProfileStore,
        showing: Showing,
    ) -> Result<(), SessionError> {
        let Some(new_profile) = &self.new_profile else {
            return Ok(());
        };
        match (showing, &self.before) {
            (Showing::After, _) => store.associate(&self.monitor, new_profile, true)?,
            (Showing::Before, Some(before)) => store.associate(&self.monitor, before, true)?,
            (Showing::Before, None) => store.dissociate(&self.monitor, new_profile)?,
        }
        self.showing = showing;
        Ok(())
    }

    /// The new profile stays, or becomes again, the default. `before` stays
    /// associated, so it is a click away in Windows' colour settings.
    pub fn keep(
        self,
        store: &mut dyn ProfileStore,
        restore: &RestoreFile,
    ) -> Result<(), SessionError> {
        if let Some(before) = &self.before {
            store.associate(&self.monitor, before, false)?;
        }
        if let Some(new_profile) = &self.new_profile {
            store.associate(&self.monitor, new_profile, true)?;
        }
        restore.clear()
    }

    /// Back to `before` and the original gamma ramp, with the new profile
    /// dissociated. Used for Discard, cancel and errors.
    ///
    /// Every step is tried even when an earlier one fails, so as much as
    /// possible is put back. The first error is returned, and the restore
    /// file then stays for the next start.
    pub fn restore(
        self,
        store: &mut dyn ProfileStore,
        restore: &RestoreFile,
    ) -> Result<(), SessionError> {
        let monitor = &self.monitor;
        let mut results = Vec::new();
        if let Some(new_profile) = &self.new_profile {
            results.push(store.dissociate(monitor, new_profile));
        }
        if let Some(before) = &self.before {
            results.push(store.associate(monitor, before, true));
        }
        results.push(store.set_gamma_ramp(monitor, &self.original_ramp));

        results.into_iter().collect::<Result<(), Error>>()?;
        restore.clear()
    }
}

/// At startup: re-applies a record left by a run that never finished, and
/// returns the message for the Ready screen. A record for a display that
/// isn't connected is kept for a later start.
pub fn recover(
    store: &mut dyn ProfileStore,
    restore: &RestoreFile,
    monitors: &[Monitor],
) -> Option<String> {
    let record = restore.load()?;
    let profile_text = record.profile.as_deref().unwrap_or("no profile");
    let Some(monitor) = monitors.iter().find(|m| m.device == record.device) else {
        let device = &record.device;
        return Some(format!(
            "Couldn't restore {profile_text}: display {device} is not connected"
        ));
    };

    let name = &monitor.name;
    if let Err(e) = reapply(store, monitor, record.profile.as_deref()) {
        return Some(format!("Couldn't restore {profile_text} on {name}: {e}"));
    }
    let restored = match &record.profile {
        Some(profile) => format!("Restored profile {profile} on {name}"),
        None => format!("Set {name} back to no profile"),
    };
    if let Err(e) = restore.clear() {
        return Some(format!("{restored}, but {e}"));
    }
    Some(format!("{restored} after an interrupted run"))
}

/// Makes `profile` the display's default again. With none, whatever is the
/// default now is dissociated.
fn reapply(
    store: &mut dyn ProfileStore,
    monitor: &Monitor,
    profile: Option<&str>,
) -> Result<(), Error> {
    if let Some(profile) = profile {
        return store.associate(monitor, profile, true);
    }
    let Some(current) = store.default_profile(monitor)? else {
        return Ok(());
    };
    store.dissociate(monitor, &current)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A profile store in memory. Dissociating the default clears it, where
    /// Windows may pick another associated profile instead
    /// (**? to be validated**).
    struct FakeStore {
        default: Option<String>,
        associated: Vec<String>,
        ramp: GammaRamp,
        /// The one call that fails, such as `"dissociate"`.
        failing: Option<&'static str>,
    }

    impl FakeStore {
        fn fail_if(&self, call: &'static str) -> Result<(), Error> {
            if self.failing == Some(call) {
                return Err(Error::Os(call, 5));
            }
            Ok(())
        }
    }

    impl ProfileStore for FakeStore {
        fn default_profile(&mut self, _monitor: &Monitor) -> Result<Option<String>, Error> {
            Ok(self.default.clone())
        }

        fn associate(
            &mut self,
            _monitor: &Monitor,
            profile: &str,
            make_default: bool,
        ) -> Result<(), Error> {
            self.fail_if("associate")?;
            if !self.associated.iter().any(|p| p == profile) {
                self.associated.push(profile.to_string());
            }
            if make_default {
                self.default = Some(profile.to_string());
            }
            Ok(())
        }

        fn dissociate(&mut self, _monitor: &Monitor, profile: &str) -> Result<(), Error> {
            self.fail_if("dissociate")?;
            self.associated.retain(|p| p != profile);
            if self.default.as_deref() == Some(profile) {
                self.default = None;
            }
            Ok(())
        }

        fn install(&mut self, path: &Path) -> Result<String, Error> {
            let name = path.file_name().unwrap_or_default();
            Ok(name.to_string_lossy().into_owned())
        }

        fn gamma_ramp(&mut self, _monitor: &Monitor) -> Result<GammaRamp, Error> {
            Ok(self.ramp)
        }

        fn set_gamma_ramp(&mut self, _monitor: &Monitor, ramp: &GammaRamp) -> Result<(), Error> {
            self.fail_if("set_gamma_ramp")?;
            self.ramp = *ramp;
            Ok(())
        }
    }

    /// A restore file in a directory of its own, so tests can run in
    /// parallel.
    fn fresh_restore_file() -> RestoreFile {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let process = std::process::id();
        let dir = std::env::temp_dir().join(format!("colibri-session-test-{process}-{count}"));
        RestoreFile {
            path: dir.join("restore.json"),
        }
    }

    /// A gamma ramp that isn't identity, as a loaded calibration leaves it.
    fn calibrated_ramp() -> GammaRamp {
        let mut ramp = identity_ramp();
        ramp[2][128] = 30_000;
        ramp
    }

    fn fake_with(profile: &str) -> (FakeStore, RestoreFile) {
        let store = FakeStore {
            default: Some(profile.to_string()),
            associated: vec![profile.to_string()],
            ramp: calibrated_ramp(),
            failing: None,
        };
        (store, fresh_restore_file())
    }

    fn fake_without_profile() -> (FakeStore, RestoreFile) {
        let store = FakeStore {
            default: None,
            associated: Vec::new(),
            ramp: calibrated_ramp(),
            failing: None,
        };
        (store, fresh_restore_file())
    }

    fn monitor() -> Monitor {
        Monitor {
            index: 0,
            device: r"\\.\DISPLAY1".into(),
            name: "ASUS PA328CGV".into(),
            source: None,
            left: 0,
            top: 0,
            width: 2560,
            height: 1440,
            primary: true,
        }
    }

    #[test]
    fn begin_takes_display_out_of_colour_management() {
        let (mut store, restore) = fake_with("factory.icc");
        DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        assert_eq!(store.default, None);
        assert!(!store.associated.contains(&"factory.icc".to_string()));
        assert_eq!(store.ramp, identity_ramp());
        assert_eq!(
            restore.load(),
            Some(RestoreRecord {
                device: monitor().device,
                profile: Some("factory.icc".into())
            })
        );
    }

    #[test]
    fn restore_after_cancel_puts_everything_back() {
        let (mut store, restore) = fake_with("factory.icc");
        let original_ramp = store.ramp;
        let session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session.restore(&mut store, &restore).unwrap();
        assert_eq!(store.default.as_deref(), Some("factory.icc"));
        assert_eq!(store.ramp, original_ramp, "gamma ramp not restored");
        assert_eq!(restore.load(), None);
    }

    #[test]
    fn switching_flips_the_default_profile() {
        let (mut store, restore) = fake_with("factory.icc");
        let mut session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session
            .install_new(&mut store, Path::new("colibri-new.icc"))
            .unwrap();
        assert_eq!(store.default.as_deref(), Some("colibri-new.icc"));
        session.show(&mut store, Showing::Before).unwrap();
        assert_eq!(store.default.as_deref(), Some("factory.icc"));
        session.show(&mut store, Showing::After).unwrap();
        assert_eq!(store.default.as_deref(), Some("colibri-new.icc"));
    }

    #[test]
    fn before_without_a_profile_means_none() {
        let (mut store, restore) = fake_without_profile();
        let mut session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session
            .install_new(&mut store, Path::new("colibri-new.icc"))
            .unwrap();
        session.show(&mut store, Showing::Before).unwrap();
        assert_eq!(store.default, None);
    }

    #[test]
    fn discard_restores_the_previous_profile() {
        let (mut store, restore) = fake_with("factory.icc");
        let mut session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session
            .install_new(&mut store, Path::new("colibri-new.icc"))
            .unwrap();
        session.restore(&mut store, &restore).unwrap();
        assert_eq!(store.default.as_deref(), Some("factory.icc"));
        assert!(!store.associated.contains(&"colibri-new.icc".to_string()));
        assert_eq!(restore.load(), None);
    }

    #[test]
    fn keep_leaves_new_default_and_old_associated() {
        let (mut store, restore) = fake_with("factory.icc");
        let mut session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session
            .install_new(&mut store, Path::new("colibri-new.icc"))
            .unwrap();
        session.keep(&mut store, &restore).unwrap();
        assert_eq!(store.default.as_deref(), Some("colibri-new.icc"));
        assert!(store.associated.contains(&"factory.icc".to_string()));
        assert_eq!(restore.load(), None);
    }

    #[test]
    fn recover_applies_a_leftover_record() {
        let (mut store, restore) = fake_without_profile();
        let record = RestoreRecord {
            device: monitor().device,
            profile: Some("factory.icc".into()),
        };
        restore.save(&record).unwrap();
        let message = recover(&mut store, &restore, &[monitor()]).unwrap();
        assert_eq!(store.default.as_deref(), Some("factory.icc"));
        assert!(message.contains("factory.icc"), "{message}");
        assert_eq!(restore.load(), None);
    }

    #[test]
    fn recover_keeps_record_for_missing_display() {
        let (mut store, restore) = fake_without_profile();
        let record = RestoreRecord {
            device: r"\\.\DISPLAY9".into(),
            profile: Some("factory.icc".into()),
        };
        restore.save(&record).unwrap();
        let message = recover(&mut store, &restore, &[monitor()]).unwrap();
        assert!(message.contains("not connected"), "{message}");
        assert_eq!(
            restore.load(),
            Some(record),
            "record dropped for a missing display"
        );
    }

    #[test]
    fn keep_while_showing_before_keeps_the_new_profile() {
        let (mut store, restore) = fake_with("factory.icc");
        let mut session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session
            .install_new(&mut store, Path::new("colibri-new.icc"))
            .unwrap();
        session.show(&mut store, Showing::Before).unwrap();
        session.keep(&mut store, &restore).unwrap();
        assert_eq!(store.default.as_deref(), Some("colibri-new.icc"));
    }

    #[test]
    fn restore_carries_on_after_a_failure() {
        let (mut store, restore) = fake_with("factory.icc");
        let original_ramp = store.ramp;
        let mut session = DisplaySession::begin(&mut store, &restore, monitor(), true).unwrap();
        session
            .install_new(&mut store, Path::new("colibri-new.icc"))
            .unwrap();
        store.failing = Some("dissociate");

        let result = session.restore(&mut store, &restore);

        assert!(result.is_err(), "the dissociate failure was lost");
        assert_eq!(store.default.as_deref(), Some("factory.icc"));
        assert_eq!(store.ramp, original_ramp, "gamma ramp not restored");
        assert!(
            restore.load().is_some(),
            "restore file removed after a failure"
        );
    }

    #[test]
    fn failed_begin_puts_back_what_it_changed() {
        let (mut store, restore) = fake_with("factory.icc");
        store.failing = Some("set_gamma_ramp");

        let result = DisplaySession::begin(&mut store, &restore, monitor(), true);

        assert!(result.is_err());
        assert_eq!(store.default.as_deref(), Some("factory.icc"));
    }

    #[test]
    fn recover_without_a_profile_removes_the_default() {
        let (mut store, restore) = fake_with("colibri-new.icc");
        let record = RestoreRecord {
            device: monitor().device,
            profile: None,
        };
        restore.save(&record).unwrap();
        let message = recover(&mut store, &restore, &[monitor()]).unwrap();
        assert_eq!(store.default, None);
        assert!(message.contains("no profile"), "{message}");
        assert_eq!(restore.load(), None);
    }
}
