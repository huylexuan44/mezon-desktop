use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let protoc_include = protoc_bin_vendored::include_path()?;
    unsafe {
        env::set_var("PROTOC", &protoc);
        env::set_var("PROTOC_INCLUDE", &protoc_include);
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    let proto_root = manifest_dir.join("../../mezon-protocol").canonicalize()?;
    let api_proto = proto_root.join("api/api.proto");
    let realtime_proto = proto_root.join("rtapi/realtime.proto");

    if !api_proto.is_file() || !realtime_proto.is_file() {
        return Err("mezon-protocol submodule is missing or incomplete".into());
    }

    let previous_dir = env::current_dir()?;
    env::set_current_dir(&proto_root)?;

    let compile_result = prost_build::Config::new()
        .protoc_executable(protoc)
        .out_dir(&out_dir)
        .compile_protos(
            &["api/api.proto", "rtapi/realtime.proto"],
            std::slice::from_ref(&PathBuf::from(".")),
        );

    env::set_current_dir(previous_dir)?;
    compile_result?;

    println!("cargo:rerun-if-changed={}", api_proto.display());
    println!("cargo:rerun-if-changed={}", realtime_proto.display());

    Ok(())
}
