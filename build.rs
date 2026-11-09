// exe에 아이콘 리소스를 넣는다 (Windows SDK의 rc.exe 사용)
fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/tray-busy.ico");
    println!("cargo:rerun-if-changed=assets/tray-paused.ico");
    embed_resource::compile("assets/app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
