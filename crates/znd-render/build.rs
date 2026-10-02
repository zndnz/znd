fn main() {
    println!("cargo:rerun-if-changed=src/plugins/ofx_shim.c");
    cc::Build::new().file("src/plugins/ofx_shim.c").warnings(true).compile("znd_ofx_shim");
}
