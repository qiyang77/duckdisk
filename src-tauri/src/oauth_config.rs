pub const ONEDRIVE_CLIENT_ID: &str = "c3ecb115-5bd3-4295-a612-898938f75dce";
pub const GOOGLE_DRIVE_CLIENT_ID: &str =
    "1012996051453-5fub1j4uce77todjq2l5rig23ameg1tn.apps.googleusercontent.com";

pub fn onedrive_client_id() -> &'static str {
    option_env!("DUCKDISK_ONEDRIVE_CLIENT_ID")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ONEDRIVE_CLIENT_ID)
}

pub fn google_drive_client_id() -> &'static str {
    option_env!("DUCKDISK_GOOGLE_CLIENT_ID")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(GOOGLE_DRIVE_CLIENT_ID)
}
