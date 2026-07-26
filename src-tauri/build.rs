fn main() {
    println!("cargo:rerun-if-env-changed=DUCKDISK_ONEDRIVE_CLIENT_ID");
    let client_id = std::env::var("DUCKDISK_ONEDRIVE_CLIENT_ID").unwrap_or_default();
    println!("cargo:rustc-env=DUCKDISK_ONEDRIVE_CLIENT_ID={client_id}");
    tauri_build::build()
}
