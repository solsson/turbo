//! Cache-key compensation for dependency edges dropped by `--only`.
//!
//! `--only` runs just the tasks matched by the filter, discarding dependency
//! edges that point outside the filter set. Those upstream tasks then never
//! execute and never produce a task hash, so nothing about them reaches the
//! downstream task's cache key. A change in an excluded package therefore
//! produces a cache *hit* on the downstream task, and turbo happily restores a
//! stale artifact.
//!
//! Upstream treats the edge dropping as intended behavior for `--only`, and we
//! agree -- changing which nodes survive the filter would change documented
//! semantics. What we do instead is keep the graph shape untouched and repair
//! the *cache key*: for every dropped edge we hash the excluded package's
//! source files and feed that in where the missing task hash would have gone.
//!
//! Known limitation: this can only compensate for edges that existed. A package
//! that does not define the depended-on task never produces a `^task` edge, so
//! there is nothing to drop and nothing to record. Use `$TURBO_ROOT$` inputs to
//! track those.

use std::{collections::HashMap, sync::Arc};

use turbopath::AbsoluteSystemPath;
use turborepo_hash::{FileHashes, TurboHash};
use turborepo_repository::package_graph::{PackageGraph, PackageName};
use turborepo_scm::{RepoGitIndex, SCM};
use turborepo_task_id::TaskId;

use crate::engine::Engine;

/// A downstream task and the dependency edges `--only` removed from it.
pub struct DroppedDependencies {
    pub task_id: TaskId<'static>,
    pub dropped: Vec<TaskId<'static>>,
}

/// Per-task stand-in hashes for dependencies that `--only` dropped.
///
/// Keyed by the downstream task. Each entry is folded into that task's
/// dependency hash list, taking the place of the task hashes the excluded
/// dependencies would have contributed.
pub type DroppedDependencyHashes = HashMap<TaskId<'static>, Vec<Arc<str>>>;

/// Hashes the packages behind every `--only`-dropped dependency edge.
///
/// Returns the per-task hash material and the set of dropped edges, the latter
/// so the caller can warn about the reduced graph.
pub fn compute_dropped_dependency_hashes(
    engine: &Engine,
    package_graph: &PackageGraph,
    scm: &SCM,
    repo_root: &AbsoluteSystemPath,
    repo_index: Option<&RepoGitIndex>,
) -> (DroppedDependencyHashes, Vec<DroppedDependencies>) {
    let mut hashes = DroppedDependencyHashes::new();
    let mut warnings = Vec::new();
    // Several tasks in the same package commonly drop the same upstream
    // package, and hashing a package walks its whole file tree, so memoize.
    let mut package_hashes: HashMap<PackageName, Option<Arc<str>>> = HashMap::new();

    for task_id in engine.task_ids() {
        let Some(dropped) = engine.dropped_dependencies(task_id) else {
            continue;
        };
        if dropped.is_empty() {
            continue;
        }

        let mut material: Vec<Arc<str>> = Vec::with_capacity(dropped.len());
        for dropped_task_id in dropped {
            let package_name = PackageName::from(dropped_task_id.package());
            let package_hash = package_hashes
                .entry(package_name.clone())
                .or_insert_with(|| {
                    hash_package(package_graph, scm, repo_root, repo_index, &package_name)
                })
                .clone();

            let Some(package_hash) = package_hash else {
                continue;
            };
            // Tag with the task id so that two different dropped tasks in the
            // same package stay distinguishable, matching how a real dependency
            // hash is specific to a task rather than to a package.
            material.push(Arc::from(
                format!("{dropped_task_id}:{package_hash}").as_str(),
            ));
        }

        if material.is_empty() {
            continue;
        }
        material.sort_unstable();
        material.dedup();

        tracing::debug!(
            "--only: folding {} dropped dependency hashes into {}",
            material.len(),
            task_id
        );
        hashes.insert(task_id.clone(), material);
        warnings.push(DroppedDependencies {
            task_id: task_id.clone(),
            dropped: dropped.to_vec(),
        });
    }

    warnings.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    (hashes, warnings)
}

/// Hashes every source file in a package, or `None` if the package is unknown
/// or unreadable.
///
/// A failure here must not fail the run: the worst case is that we fall back to
/// the uncompensated hash, which is exactly upstream's behavior.
fn hash_package(
    package_graph: &PackageGraph,
    scm: &SCM,
    repo_root: &AbsoluteSystemPath,
    repo_index: Option<&RepoGitIndex>,
    package_name: &PackageName,
) -> Option<Arc<str>> {
    let package_info = package_graph.package_info(package_name)?;
    let package_path = package_info.package_path();

    let hashes = scm
        .get_package_file_hashes(
            repo_root,
            package_path,
            &[] as &[&str],
            true,
            None,
            repo_index,
        )
        .inspect_err(|err| {
            tracing::debug!("--only: could not hash package {package_name}: {err}");
        })
        .ok()?;

    let mut pairs: Vec<_> = hashes.into_iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Some(Arc::from(FileHashes(pairs).hash().as_str()))
}

/// Renders the user-facing warning for a set of dropped edges.
pub fn dropped_dependency_warning(dropped: &[DroppedDependencies]) -> Option<String> {
    if dropped.is_empty() {
        return None;
    }

    let mut packages: Vec<&str> = dropped
        .iter()
        .flat_map(|entry| entry.dropped.iter().map(|task_id| task_id.package()))
        .collect();
    packages.sort_unstable();
    packages.dedup();

    let filters = packages
        .iter()
        .map(|package| format!("--filter={package}"))
        .collect::<Vec<_>>()
        .join(" ");

    Some(format!(
        "`--only` dropped {} dependency task(s) from the task graph. Their package sources are \
         still hashed into the cache key, so changes to them will invalidate correctly. To run \
         them as well, add: {filters}\nNote that packages which do not define the depended-on \
         task are not tracked this way; use $TURBO_ROOT$ inputs for those.",
        dropped
            .iter()
            .map(|entry| entry.dropped.len())
            .sum::<usize>(),
    ))
}
