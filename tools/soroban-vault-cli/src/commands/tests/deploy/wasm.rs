use super::*;

#[test]
fn deploy_wasm_build_embeds_source_repo_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let governance_path = ArtifactSpec::from_name(ArtifactName::Governance).wasm_path(dir.path());
    fs::create_dir_all(governance_path.parent().expect("parent")).expect("create parent");
    fs::write(&governance_path, "governance").expect("write wasm");
    fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Wasm(crate::cli::DeployWasmArgs {
                artifact: ArtifactName::Governance,
                build: true,
            }),
        }),
        ..base_cli(dir.path().join("manifest.json"), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy wasm");

    let calls = executor.calls();
    let build = calls
        .iter()
        .find(|(_, args)| args.windows(2).any(|pair| pair == ["contract", "build"]))
        .expect("build command should run");
    assert!(build
        .1
        .windows(2)
        .any(|pair| pair == ["--meta", "source_repo=github:Templar-Protocol/contracts"]));
}
