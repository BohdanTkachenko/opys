//! Role queues across primary corpora. Failed corpora remain errors, never zeroes.

use std::collections::BTreeMap;

use opys_engine::queue::QueueResult;
use serde::Serialize;

use crate::actor::RoleQueue;

/// One project's answer, in the sidebar's project order.
pub struct ProjectQueues {
    pub key: String,
    pub label: String,
    pub queues: Result<Vec<RoleQueue>, String>,
}

#[derive(Debug, Serialize)]
pub struct RolesView {
    pub schema: &'static str,
    pub roles: Vec<RoleSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ProjectError>,
}

#[derive(Debug, Serialize)]
pub struct RoleSummary {
    pub name: String,
    /// Present only when every declaring project agrees on a known kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<opys_engine::project_config::RoleKind>,
    pub projects: Vec<RoleProject>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct RoleProject {
    pub key: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<opys_engine::project_config::RoleKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct ProjectError {
    pub key: String,
    pub label: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct RoleView {
    pub schema: &'static str,
    pub role: String,
    pub projects: Vec<ProjectQueue>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ProjectQueue {
    Ready {
        key: String,
        label: String,
        queue: QueueResult,
    },
    Error(ProjectError),
}

/// Exact-name merge; the ordered map sorts names while preserving project order.
pub fn roles(projects: Vec<ProjectQueues>) -> RolesView {
    let mut roles: BTreeMap<String, RoleSummary> = BTreeMap::new();
    let mut errors = Vec::new();
    for project in projects {
        match project.queues {
            Err(error) => errors.push(ProjectError {
                key: project.key,
                label: project.label,
                error,
            }),
            Ok(queues) => {
                for role in queues {
                    let entry = roles
                        .entry(role.name.clone())
                        .or_insert_with(|| RoleSummary {
                            name: role.name,
                            kind: role.kind,
                            projects: Vec::new(),
                            total: 0,
                        });
                    if entry.kind != role.kind {
                        entry.kind = None;
                    }
                    entry.total += role.queue.total;
                    entry.projects.push(RoleProject {
                        key: project.key.clone(),
                        label: project.label.clone(),
                        kind: role.kind,
                        description: role.description,
                        total: role.queue.total,
                    });
                }
            }
        }
    }
    RolesView {
        schema: "opys.roles/1",
        roles: roles.into_values().collect(),
        errors,
    }
}

/// A project that cannot answer cannot establish whether it declares the role.
/// Keep its error visible; only an answered declaration can exclude a project.
pub fn role(projects: Vec<ProjectQueues>, name: String) -> RoleView {
    let projects = projects
        .into_iter()
        .filter_map(|project| match project.queues {
            Err(error) => Some(ProjectQueue::Error(ProjectError {
                key: project.key,
                label: project.label,
                error,
            })),
            Ok(queues) => queues
                .into_iter()
                .find(|role| role.name == name)
                .map(|role| ProjectQueue::Ready {
                    key: project.key,
                    label: project.label,
                    queue: role.queue,
                }),
        })
        .collect();
    RoleView {
        schema: "opys.role/1",
        role: name,
        projects,
    }
}
