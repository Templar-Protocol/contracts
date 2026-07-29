use super::*;

#[test]
fn doctor_checks_stellar_and_source_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
    fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
    let config_dir = dir.path().join("stellar-config");
    let cli = Cli {
        workspace_path: dir.path().into(),
        config_dir: Some(config_dir.clone()),
        command: Commands::Doctor,
        ..base_cli(dir.path().join("manifest.json"), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run doctor");

    let calls = executor.calls();
    assert!(calls
        .iter()
        .any(|(_, args)| args == &["--version".to_string()]));
    assert!(calls.iter().any(|(_, args)| args
        == &[
            "keys".to_string(),
            "address".to_string(),
            "alice".to_string(),
            "--config-dir".to_string(),
            config_dir.display().to_string(),
        ]));
}
