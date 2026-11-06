// Puts the Android runner APK (android/runner, built with Gradle first) inside the binary, so an app without the
// library needs nothing installed by hand. A build without it still works; `--device` on Android then says how to
// build it.
use std::path::Path;

fn main() {
    let apk = Path::new("../android/runner/build/outputs/apk/debug/runner-debug.apk");
    println!("cargo:rerun-if-changed={}", apk.display());
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("runner.apk");
    std::fs::write(&out, std::fs::read(apk).unwrap_or_default()).unwrap();
}
