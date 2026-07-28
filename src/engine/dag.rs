//! Step DAG construction per play (PRD §4/§8): `requires` edges feed a
//! petgraph graph; cycles are rejected at validate time. The scheduler
//! consumes the DAG; reporting always uses declaration order.
//!
//! Names are resolved by *scope* rather than play-globally, because
//! composite expansion can put two steps with the same name in one play.
//! A step declared in the playbook sees the playbook's own steps; a step
//! expanded from a composite sees only its siblings in that same
//! invocation. Requiring an invocation by name depends on every step it
//! expanded into — "after the whole block".

use petgraph::algo::{is_cyclic_directed, kosaraju_scc};
use petgraph::graph::{DiGraph, NodeIndex};

use crate::diag::Diag;
use crate::model::{Play, Step};

/// Dependency view of one play. Indices refer to the play's flattened
/// declaration-order step list (`Play::steps()`).
pub struct StepDag {
    /// For each step, the indices of the steps it requires.
    pub deps: Vec<Vec<usize>>,
}

/// Resolve one `requires` name from a scope, as the indices it covers.
///
/// A scope is a `container_path` prefix plus the composite depth at which
/// the name was written: depth 0 is the playbook, depth *d* the body of the
/// *d*-th enclosing composite. Returns an empty vec when nothing matches —
/// unknown names are reported at load time, so the DAG just drops the edge.
fn resolve(steps: &[&Step], scope: &[String], depth: usize, req: &str) -> Vec<usize> {
    if depth == 0 {
        // Playbook scope is flat across containers, as it has always been:
        // a step requires another step of the same play by name.
        if let Some((i, _)) = steps
            .iter()
            .enumerate()
            .find(|(_, s)| s.frames.is_empty() && s.name == req)
        {
            return vec![i];
        }
        // Or it names a composite invocation, covering every step that
        // invocation expanded into.
        return steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.frames.first().is_some_and(|f| f.step == req))
            .map(|(i, _)| i)
            .collect();
    }

    // Inside a composite, only siblings of the same invocation are
    // reachable — that is what makes a body a self-contained unit.
    if let Some((i, _)) = steps
        .iter()
        .enumerate()
        .find(|(_, s)| s.frames.len() == depth && s.name == req && s.container_path == scope)
    {
        return vec![i];
    }
    let mut group_path = scope.to_vec();
    group_path.push(req.to_string());
    steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.frames.len() > depth && s.container_path.starts_with(&group_path))
        .map(|(i, _)| i)
        .collect()
}

pub fn build(play: &Play) -> Result<StepDag, Vec<Diag>> {
    let steps: Vec<&Step> = play.steps();

    let mut graph: DiGraph<usize, ()> = DiGraph::new();
    let nodes: Vec<NodeIndex> = (0..steps.len()).map(|i| graph.add_node(i)).collect();

    let mut deps = vec![Vec::new(); steps.len()];
    for (i, step) in steps.iter().enumerate() {
        // The step's own `requires`, in its own scope, plus the `requires`
        // of every invocation that produced it — each resolved in the scope
        // of whoever wrote it.
        let own = (
            step.container_path.as_slice(),
            step.frames.len(),
            &step.requires,
        );
        let inherited = step.frames.iter().enumerate().map(|(d, f)| {
            let cut = step.container_path.len() - (step.frames.len() - d);
            (&step.container_path[..cut], d, &f.requires)
        });
        for (scope, depth, requires) in std::iter::once(own).chain(inherited) {
            for req in requires {
                for j in resolve(&steps, scope, depth, req) {
                    if j == i {
                        continue;
                    }
                    graph.add_edge(nodes[j], nodes[i], ());
                    deps[i].push(j);
                }
            }
        }
        deps[i].sort_unstable();
        deps[i].dedup();
    }

    if is_cyclic_directed(&graph) {
        let mut diags = Vec::new();
        for scc in kosaraju_scc(&graph) {
            if scc.len() > 1 {
                // Paths, not bare names: expansion makes names ambiguous.
                let mut names: Vec<String> =
                    scc.iter().map(|n| path_of(steps[graph[*n]])).collect();
                names.sort();
                diags.push(Diag::bare(format!(
                    "dependency cycle in play '{}' between steps: {}",
                    play.name,
                    names.join(" -> ")
                )));
            }
        }
        if diags.is_empty() {
            diags.push(Diag::bare(format!(
                "dependency cycle in play '{}'",
                play.name
            )));
        }
        return Err(diags);
    }

    Ok(StepDag { deps })
}

fn path_of(step: &Step) -> String {
    let mut parts = step.container_path.clone();
    parts.push(step.name.clone());
    parts.join("/")
}
