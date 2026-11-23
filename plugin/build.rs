fn main() {
    println!("cargo:rerun-if-env-changed=WORLDPORT_NATIVE_COMPONENTS");
    println!("cargo:rerun-if-env-changed=BMCBL_PLUGIN_PACKAGE_OUT_DIR");
    if let Err(error) = bmcbl_plugin_api::pack::auto_pack() {
        println!("cargo:warning=WorldPort plugin auto-pack failed: {error:#}");
        std::process::exit(1);
    }
}
