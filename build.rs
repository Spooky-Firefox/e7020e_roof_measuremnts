use std::{fs, path::Path};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_opencv_algorithm_hint)");

    if detect_algorithm_hint_support() {
        println!("cargo::rustc-cfg=has_opencv_algorithm_hint");
    }
}

fn detect_algorithm_hint_support() -> bool {
    opencv_include_paths()
        .iter()
        .any(|include_path| include_path_has_algorithm_hint(include_path))
}

fn opencv_include_paths() -> Vec<std::path::PathBuf> {
    probe_include_paths("opencv4")
        .unwrap_or_else(|_| probe_include_paths("opencv").unwrap_or_default())
}

fn probe_include_paths(package: &str) -> Result<Vec<std::path::PathBuf>, pkg_config::Error> {
    let library = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe(package)?;
    Ok(library.include_paths)
}

fn include_path_has_algorithm_hint(include_path: &Path) -> bool {
    [
        "opencv2/core.hpp",
        "opencv2/core/base.hpp",
        "opencv2/core/utility.hpp",
        "opencv4/opencv2/core.hpp",
        "opencv4/opencv2/core/base.hpp",
        "opencv4/opencv2/core/utility.hpp",
    ]
    .iter()
    .filter_map(|relative_path| fs::read_to_string(include_path.join(relative_path)).ok())
    .any(|contents| contents.contains("AlgorithmHint"))
}
