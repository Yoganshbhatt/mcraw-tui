fn main() {
    let decoder_dir = "motioncam-decoder";

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/c_api.cpp")
        .file(format!("{}/lib/Decoder.cpp", decoder_dir))
        .file(format!("{}/lib/RawData.cpp", decoder_dir))
        .file(format!("{}/lib/RawData_Legacy.cpp", decoder_dir))
        .include(format!("{}/lib/include", decoder_dir))
        .include(format!("{}/thirdparty", decoder_dir))
        .include("src")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-sign-compare")
        .flag_if_supported("-fexceptions")
        .compile("mc_c_api");

    println!("cargo:rerun-if-changed=src/c_api.cpp");
    println!("cargo:rerun-if-changed=src/c_api.h");
}
