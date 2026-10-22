fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/com.chrisdaggas.tunnelmanager.gresource.xml",
        "tunnel-manager.gresource",
    );
}
