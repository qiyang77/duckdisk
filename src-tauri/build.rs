fn main() {
    println!("cargo:rerun-if-env-changed=DUCKDISK_ONEDRIVE_CLIENT_ID");
    let client_id = std::env::var("DUCKDISK_ONEDRIVE_CLIENT_ID").unwrap_or_default();
    println!("cargo:rustc-env=DUCKDISK_ONEDRIVE_CLIENT_ID={client_id}");
    println!("cargo:rerun-if-env-changed=DUCKDISK_GOOGLE_CLIENT_ID");
    let google_client_id = std::env::var("DUCKDISK_GOOGLE_CLIENT_ID").unwrap_or_default();
    println!("cargo:rustc-env=DUCKDISK_GOOGLE_CLIENT_ID={google_client_id}");
    println!("cargo:rerun-if-env-changed=DUCKDISK_GOOGLE_CLIENT_SECRET");
    let google_client_secret = std::env::var("DUCKDISK_GOOGLE_CLIENT_SECRET").unwrap_or_default();
    println!("cargo:rustc-env=DUCKDISK_GOOGLE_CLIENT_SECRET={google_client_secret}");
    tauri_build::build()
}
