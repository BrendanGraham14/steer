use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use tempfile::TempDir;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use steer_workspace::local::LocalWorkspace;
use steer_workspace::{GrepRequest, Workspace, WorkspaceOpContext};

const FILES_IN_HIGH_DENSITY: usize = 100;
const FILES_IN_SPARSE: usize = 120;
const FILES_IN_NO_MATCH: usize = 120;
const LINES_PER_FILE: usize = 120;

struct BenchDataset {
    _root: TempDir,
    workspace: LocalWorkspace,
}

impl BenchDataset {
    fn path(&self) -> &Path {
        self._root.path()
    }
}

fn write_file(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap_or_else(|err| {
        panic!("failed to write fixture file {}: {err}", path.display());
    });
}

fn build_high_density(rt: &Runtime) -> BenchDataset {
    let root = tempfile::tempdir().expect("failed to create high_density tempdir");

    for i in 0..FILES_IN_HIGH_DENSITY {
        let file_path = root.path().join(format!("src/high_density_{i:03}.rs"));
        std::fs::create_dir_all(file_path.parent().expect("file has parent"))
            .expect("failed to create fixture dirs");

        let mut content = String::new();
        for line in 0..LINES_PER_FILE {
            content.push_str(&format!(
                "let value_{line} = \"needle needle marker {i} {line}\";\n"
            ));
        }
        write_file(&file_path, &content);
    }

    let workspace = rt
        .block_on(LocalWorkspace::with_path(root.path().to_path_buf()))
        .expect("failed to create LocalWorkspace for high_density dataset");

    BenchDataset {
        _root: root,
        workspace,
    }
}

fn build_sparse(rt: &Runtime) -> BenchDataset {
    let root = tempfile::tempdir().expect("failed to create sparse tempdir");

    for i in 0..FILES_IN_SPARSE {
        let file_path = root.path().join(format!("src/sparse_{i:03}.rs"));
        std::fs::create_dir_all(file_path.parent().expect("file has parent"))
            .expect("failed to create fixture dirs");

        let mut content = String::new();
        for line in 0..LINES_PER_FILE {
            if line == 0 {
                content.push_str(&format!("const MATCH: &str = \"needle sparse {i}\";\n"));
            } else {
                content.push_str("const NO_MATCH: &str = \"other text\";\n");
            }
        }
        write_file(&file_path, &content);
    }

    let workspace = rt
        .block_on(LocalWorkspace::with_path(root.path().to_path_buf()))
        .expect("failed to create LocalWorkspace for sparse dataset");

    BenchDataset {
        _root: root,
        workspace,
    }
}

fn build_no_match(rt: &Runtime) -> BenchDataset {
    let root = tempfile::tempdir().expect("failed to create no_match tempdir");

    for i in 0..FILES_IN_NO_MATCH {
        let file_path = root.path().join(format!("src/no_match_{i:03}.rs"));
        std::fs::create_dir_all(file_path.parent().expect("file has parent"))
            .expect("failed to create fixture dirs");

        let mut content = String::new();
        for line in 0..LINES_PER_FILE {
            content.push_str(&format!(
                "let value_{line} = \"unrelated content {i} {line}\";\n"
            ));
        }
        write_file(&file_path, &content);
    }

    let workspace = rt
        .block_on(LocalWorkspace::with_path(root.path().to_path_buf()))
        .expect("failed to create LocalWorkspace for no_match dataset");

    BenchDataset {
        _root: root,
        workspace,
    }
}

fn build_include_filtered(rt: &Runtime) -> BenchDataset {
    let root = tempfile::tempdir().expect("failed to create include_filtered tempdir");

    for i in 0..80usize {
        let rs_path = root.path().join(format!("src/code_{i:03}.rs"));
        std::fs::create_dir_all(rs_path.parent().expect("file has parent"))
            .expect("failed to create fixture dirs");
        write_file(
            &rs_path,
            &format!("fn f() {{\n    let marker = \"needle rust {i}\";\n}}\n"),
        );

        let md_path = root.path().join(format!("docs/doc_{i:03}.md"));
        std::fs::create_dir_all(md_path.parent().expect("file has parent"))
            .expect("failed to create fixture dirs");
        write_file(
            &md_path,
            &format!(
                "# Doc\nThis markdown file also contains needle {i}, but should be excluded.\n"
            ),
        );
    }

    let workspace = rt
        .block_on(LocalWorkspace::with_path(root.path().to_path_buf()))
        .expect("failed to create LocalWorkspace for include_filtered dataset");

    BenchDataset {
        _root: root,
        workspace,
    }
}

fn run_grep(
    rt: &Runtime,
    workspace: &LocalWorkspace,
    path: &Path,
    pattern: &str,
    include: Option<&str>,
) {
    let request = GrepRequest {
        pattern: pattern.to_string(),
        include: include.map(std::string::ToString::to_string),
        path: Some(path.to_string_lossy().to_string()),
    };
    let context = WorkspaceOpContext::new("bench-grep", CancellationToken::new());

    let result = rt
        .block_on(workspace.grep(request, &context))
        .expect("grep benchmark request should succeed");

    assert!(
        result.search_completed,
        "benchmark grep unexpectedly cancelled"
    );
}

fn bench_grep(c: &mut Criterion) {
    let rt = Runtime::new().expect("failed to create tokio runtime for benchmarks");

    let high_density = build_high_density(&rt);
    let sparse = build_sparse(&rt);
    let no_match = build_no_match(&rt);
    let include_filtered = build_include_filtered(&rt);

    let mut group = c.benchmark_group("workspace_grep");
    group.sample_size(20);

    group.bench_function("high_match_density", |b| {
        b.iter(|| {
            run_grep(
                &rt,
                &high_density.workspace,
                high_density.path(),
                "needle",
                Some("*.rs"),
            );
        });
    });

    group.bench_function("sparse_match", |b| {
        b.iter(|| {
            run_grep(
                &rt,
                &sparse.workspace,
                sparse.path(),
                "needle",
                Some("*.rs"),
            );
        });
    });

    group.bench_function("no_match", |b| {
        b.iter(|| {
            run_grep(
                &rt,
                &no_match.workspace,
                no_match.path(),
                "needle",
                Some("*.rs"),
            );
        });
    });

    group.bench_function("include_glob_filtered", |b| {
        b.iter(|| {
            run_grep(
                &rt,
                &include_filtered.workspace,
                include_filtered.path(),
                "needle",
                Some("*.rs"),
            );
        });
    });

    group.finish();
}

criterion_group!(benches, bench_grep);
criterion_main!(benches);
