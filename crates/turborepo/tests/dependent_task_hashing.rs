mod common;

use std::fs;

use common::{combined_output, run_turbo, setup};

/// Simple case: prepare outputs generated.txt, build inputs generated.txt
/// and dependsOn prepare. Second run should be FULL TURBO.
#[test]
fn test_dependent_task_hashing_simple() {
    let tempdir = tempfile::tempdir().unwrap();
    setup::setup_integration_test(tempdir.path(), "dependent_task_hashing", "npm@10.5.0", true)
        .unwrap();

    // Run 1: cache miss for both tasks
    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=simple"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 cached, 2 total"),
        "Run 1 should have 0 cached tasks.\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run 2: both tasks should be cached
    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=simple"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("2 cached, 2 total"),
        "Run 2 should have 2 cached tasks (FULL TURBO).\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("FULL TURBO"),
        "Run 2 should report FULL TURBO.\nstdout: {stdout}"
    );
}

/// Transitive case: prepare → transform → build chain where each step
/// produces outputs consumed by the next. Second run should be FULL TURBO.
#[test]
fn test_dependent_task_hashing_transitive() {
    let tempdir = tempfile::tempdir().unwrap();
    setup::setup_integration_test(tempdir.path(), "dependent_task_hashing", "npm@10.5.0", true)
        .unwrap();

    // Run 1: cache miss for all three tasks
    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=transitive"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 cached, 3 total"),
        "Run 1 should have 0 cached tasks.\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run 2: all tasks should be cached
    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=transitive"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("3 cached, 3 total"),
        "Run 2 should have 3 cached tasks (FULL TURBO).\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("FULL TURBO"),
        "Run 2 should report FULL TURBO.\nstdout: {stdout}"
    );
}

/// Config-level info messages are emitted in the run prelude.
#[test]
fn test_dependent_task_hashing_info() {
    let tempdir = tempfile::tempdir().unwrap();
    setup::setup_integration_test(tempdir.path(), "dependent_task_hashing", "npm@10.5.0", true)
        .unwrap();

    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=simple"],
    );
    let combined = combined_output(&output);

    assert!(
        combined.contains("Task \"build\" inputs match \"prepare\" outputs"),
        "Should emit config-level info about deferred hashing.\ncombined: {combined}"
    );
}

/// When a dependency with cache:false produces different output each run,
/// the dependent task must NOT use a stale cache — its hash should change.
#[test]
fn test_dependent_task_hashing_changing_output() {
    let tempdir = tempfile::tempdir().unwrap();
    setup::setup_integration_test(tempdir.path(), "dependent_task_hashing", "npm@10.5.0", true)
        .unwrap();

    // Create a cache:false variant: prepare uses date for non-deterministic output
    let pkg_dir = tempdir.path().join("packages").join("simple");
    fs::write(
        pkg_dir.join("package.json"),
        r#"{
  "name": "simple",
  "scripts": {
    "prepare": "date +%s%N > generated.txt",
    "build": "echo Building"
  }
}
"#,
    )
    .unwrap();
    fs::write(
        pkg_dir.join("turbo.json"),
        r#"{
  "$schema": "https://turbo.build/schema.json",
  "extends": ["//"],
  "tasks": {
    "prepare": {
      "cache": false,
      "inputs": ["package.json"],
      "outputs": ["generated.txt"]
    },
    "build": {
      "inputs": ["generated.txt"],
      "dependsOn": ["prepare"]
    }
  }
}
"#,
    )
    .unwrap();
    common::git(tempdir.path(), &["add", "."]);
    common::git(tempdir.path(), &["commit", "-m", "cache:false prepare"]);

    // Run 1
    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=simple"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 cached, 2 total"),
        "Run 1 should have 0 cached.\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Small delay to ensure date produces different output
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Run 2: prepare runs again (cache:false) with different output.
    // Build must NOT cache — its input (generated.txt) changed.
    let output = run_turbo(
        tempdir.path(),
        &["run", "build", "--output-logs=none", "--filter=simple"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 cached, 2 total"),
        "Run 2 should have 0 cached (prepare output changed).\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !stdout.contains("FULL TURBO"),
        "Run 2 must NOT be FULL TURBO.\nstdout: {stdout}"
    );
}

/// A non-deferred task (checks) depends on a deferred task (schemas).
/// This must not crash and should cache correctly on the second run.
#[test]
fn test_dependent_task_hashing_depends_on_deferred() {
    let tempdir = tempfile::tempdir().unwrap();
    setup::setup_integration_test(tempdir.path(), "dependent_task_hashing", "npm@10.5.0", true)
        .unwrap();

    // Run 1: all three tasks (prepare, schemas, checks) miss
    let output = run_turbo(
        tempdir.path(),
        &[
            "run",
            "checks",
            "--output-logs=none",
            "--filter=depends-on-deferred",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0 cached, 3 total"),
        "Run 1 should have 0 cached tasks.\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run 2: all should cache
    let output = run_turbo(
        tempdir.path(),
        &[
            "run",
            "checks",
            "--output-logs=none",
            "--filter=depends-on-deferred",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("3 cached, 3 total"),
        "Run 2 should have 3 cached tasks (FULL TURBO).\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("FULL TURBO"),
        "Run 2 should report FULL TURBO.\nstdout: {stdout}"
    );
}
