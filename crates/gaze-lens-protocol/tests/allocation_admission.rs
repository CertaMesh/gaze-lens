// Keep the allocator's necessary unsafe System delegation outside the production
// crate's forbid(unsafe_code) policy. The standalone probe links this build's rlib.
#[test]
fn admission_precedes_owned_copies() {
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let mut libraries: Vec<_> = std::fs::read_dir(&deps)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("libgaze_lens_protocol-")
                && path.extension().is_some_and(|ext| ext == "rlib")
        })
        .collect();
    libraries.sort_by_key(|path| std::fs::metadata(path).unwrap().modified().unwrap());
    let library = libraries
        .last()
        .expect("protocol rlib from this Cargo build");
    let output = deps.join(format!(
        "allocation-probe-{}{}",
        std::process::id(),
        std::env::consts::EXE_SUFFIX
    ));
    let compile = std::process::Command::new("rustc")
        .args(["--edition", "2024"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/allocation_probe.rs"
        ))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()))
        .arg("--extern")
        .arg(format!("gaze_lens_protocol={}", library.display()))
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = std::process::Command::new(&output).output().unwrap();
    std::fs::remove_file(output).unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
