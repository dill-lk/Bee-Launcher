use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=BEE_LAUNCHER_PAYLOAD");
    tauri_build::build();

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"))
        .join("bee-launcher-payload.exe");
    if let Some(payload) = env::var_os("BEE_LAUNCHER_PAYLOAD") {
        let payload = PathBuf::from(payload);
        let bytes = fs::read(&payload).unwrap_or_else(|error| {
            panic!(
                "could not read launcher payload {}: {error}",
                payload.display()
            )
        });
        if bytes.len() < 2 || &bytes[..2] != b"MZ" {
            panic!(
                "launcher payload is not a Windows PE executable: {}",
                payload.display()
            );
        }
        fs::write(&out, bytes).expect("launcher payload should be copied into OUT_DIR");
    } else if env::var("PROFILE").as_deref() == Ok("release") {
        panic!("BEE_LAUNCHER_PAYLOAD must point to the built Windows Bee Launcher executable for a release setup build");
    } else {
        fs::write(&out, []).expect("empty development payload should be written");
        println!("cargo:warning=Bee Launcher setup has no embedded release payload; installation is disabled in this development build");
    }
}
