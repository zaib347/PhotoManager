//! PhotoManager's optional AI features. Nothing here runs, and nothing is downloaded,
//! until the user installs a feature; models then run on this computer's processor only
//! (photos never leave it).
//!
//! Kept out of `photo-core` so the core stays small and testable without models.

pub mod clip;
pub mod models;
pub mod runtime;
pub mod search;
pub mod tokenizer;

/// True while the computer runs on battery (indexing waits for mains power).
pub fn on_battery() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut status = SYSTEM_POWER_STATUS::default();
        // ACLineStatus: 0 offline (battery), 1 online, 255 unknown (desktops).
        unsafe { GetSystemPowerStatus(&mut status) }.is_ok() && status.ACLineStatus == 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
