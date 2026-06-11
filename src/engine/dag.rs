//! Step DAG construction per play (PRD §4/§8): `requires` edges feed a
//! petgraph graph; cycles are rejected at validate time. The scheduler
//! consumes the DAG; reporting always uses declaration order.

use std::collections::HashMap;

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

pub fn build(play: &Play) -> Result<StepDag, Vec<Diag>> {
    let steps: Vec<&Step> = play.steps();
    let index_of: HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.as_str(), i))
        .collect();

    let mut graph: DiGraph<usize, ()> = DiGraph::new();
    let nodes: Vec<NodeIndex> = (0..steps.len()).map(|i| graph.add_node(i)).collect();

    let mut deps = vec![Vec::new(); steps.len()];
    for (i, step) in steps.iter().enumerate() {
        for req in &step.requires {
            // Unknown names were already reported at load time.
            if let Some(&j) = index_of.get(req.as_str()) {
                graph.add_edge(nodes[j], nodes[i], ());
                deps[i].push(j);
            }
        }
    }

    if is_cyclic_directed(&graph) {
        let mut diags = Vec::new();
        for scc in kosaraju_scc(&graph) {
            if scc.len() > 1 {
                let mut names: Vec<&str> = scc.iter().map(|n| steps[graph[*n]].name.as_str()).collect();
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
