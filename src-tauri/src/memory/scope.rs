use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::memory::{MemoryError, MemoryScope, MemoryScopeKind, MemoryType};
use crate::persistence::{ProjectRecord, ProjectionDb};
use crate::storage::now_ms;
use crate::workbench::workspace_path_key;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryScopeOption {
    pub scope: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct MemoryScopeResolver {
    db: ProjectionDb,
}

impl MemoryScopeResolver {
    pub fn new(db: ProjectionDb) -> Self {
        Self { db }
    }

    pub fn scopes(&self, thread_id: &str) -> Result<Vec<MemoryScope>, MemoryError> {
        match self.resolve_scopes(thread_id) {
            Err(error) if error.code() == "MEM_INVALID_SCOPE" => Ok(thread_user_scopes(thread_id)),
            result => result,
        }
    }

    fn resolve_scopes(&self, thread_id: &str) -> Result<Vec<MemoryScope>, MemoryError> {
        if Uuid::parse_str(thread_id).is_err() {
            return Err(MemoryError::coded(
                "MEM_INVALID_ARGUMENT",
                "thread identity must be a host-generated UUID",
            ));
        }
        let mut scopes = thread_user_scopes(thread_id);
        let thread = self
            .db
            .list_threads()?
            .into_iter()
            .find(|thread| thread.id == thread_id);
        if let Some(thread) = thread
            && thread.in_project
            && let Some(path) = thread.workspace_path.as_deref()
        {
            let canonical = canonical_binding(path)?;
            let project_id = self.project_id(&canonical)?;
            scopes.insert(
                1,
                MemoryScope::new(MemoryScopeKind::Project, Some(project_id.clone())),
            );
            scopes.insert(
                2,
                MemoryScope::new(MemoryScopeKind::Workspace, Some(project_id)),
            );
        }
        Ok(scopes)
    }

    pub fn default_scope(
        &self,
        thread_id: &str,
        memory_type: MemoryType,
    ) -> Result<MemoryScope, MemoryError> {
        let scopes = if memory_type == MemoryType::WorkState {
            self.scopes(thread_id)?
        } else {
            self.resolve_scopes(thread_id)?
        };
        if memory_type != MemoryType::WorkState
            && let Some(project) = scopes
                .iter()
                .find(|scope| scope.kind == MemoryScopeKind::Project)
        {
            return Ok(project.clone());
        }
        Ok(scopes[0].clone())
    }

    pub fn options(&self, thread_id: &str) -> Result<Vec<MemoryScopeOption>, MemoryError> {
        let mut scopes = self.scopes(thread_id)?;
        scopes.sort_by_key(|scope| match scope.kind {
            MemoryScopeKind::User => 0,
            MemoryScopeKind::Thread => 1,
            MemoryScopeKind::Project => 2,
            MemoryScopeKind::Workspace => 3,
        });
        Ok(scopes
            .into_iter()
            .map(|scope| MemoryScopeOption {
                label: match scope.kind {
                    MemoryScopeKind::Thread => "当前会话",
                    MemoryScopeKind::Project => "当前会话绑定的项目",
                    MemoryScopeKind::Workspace => "当前会话绑定的工作区",
                    MemoryScopeKind::User => "所有项目（用户全局）",
                }
                .to_owned(),
                scope: scope.canonical(),
            })
            .collect())
    }

    pub fn validate_management_scope(&self, scope: &MemoryScope) -> Result<(), MemoryError> {
        scope.validate()?;
        if scope.kind == MemoryScopeKind::User {
            return Ok(());
        }
        let id = scope
            .id
            .as_deref()
            .ok_or_else(|| MemoryError::coded("MEM_INVALID_SCOPE", "scope identity is required"))?;
        let threads = self.db.list_threads()?;
        if scope.kind == MemoryScopeKind::Thread {
            if Uuid::parse_str(id).is_ok() && threads.iter().any(|thread| thread.id == id) {
                return Ok(());
            }
        } else {
            let projects = self.db.list_projects()?;
            for thread in threads.iter().filter(|thread| thread.in_project) {
                let Some(path) = thread.workspace_path.as_deref() else {
                    continue;
                };
                let Ok(canonical) = canonical_binding(path) else {
                    continue;
                };
                let Some(path) = canonical.to_str() else {
                    continue;
                };
                if matching_project_id(&projects, &workspace_path_key(path))?.as_deref() == Some(id)
                {
                    return Ok(());
                }
            }
        }
        Err(MemoryError::coded(
            "MEM_INVALID_SCOPE",
            "scope is not backed by a valid host thread binding",
        ))
    }

    fn project_id(&self, canonical: &Path) -> Result<String, MemoryError> {
        let path = canonical.to_str().ok_or_else(|| {
            MemoryError::coded("MEM_INVALID_SCOPE", "workspace binding is not valid UTF-8")
        })?;
        let key = workspace_path_key(path);
        if let Some(id) = matching_project_id(&self.db.list_projects()?, &key)? {
            return Ok(id);
        }
        let project = ProjectRecord {
            id: stable_project_id(&key),
            name: canonical
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("项目")
                .to_owned(),
            path: path.to_owned(),
            trusted: false,
            last_opened_at_ms: now_ms(),
        };
        self.db.upsert_project(&project)?;
        matching_project_id(&self.db.list_projects()?, &key)?.ok_or_else(|| {
            MemoryError::coded("MEM_INVALID_SCOPE", "project identity was not persisted")
        })
    }
}

fn thread_user_scopes(thread_id: &str) -> Vec<MemoryScope> {
    vec![
        MemoryScope::new(MemoryScopeKind::Thread, Some(thread_id.to_owned())),
        MemoryScope::user(),
    ]
}

fn canonical_binding(path: &str) -> Result<PathBuf, MemoryError> {
    let recorded = Path::new(path);
    if !recorded.is_absolute() {
        return Err(MemoryError::coded(
            "MEM_INVALID_SCOPE",
            "workspace binding must be absolute",
        ));
    }
    let canonical = recorded
        .canonicalize()
        .map_err(|_| MemoryError::coded("MEM_INVALID_SCOPE", "workspace binding is unavailable"))?;
    let canonical_path = canonical.to_str().ok_or_else(|| {
        MemoryError::coded("MEM_INVALID_SCOPE", "workspace binding is not valid UTF-8")
    })?;
    if workspace_path_key(path) != workspace_path_key(canonical_path) {
        return Err(MemoryError::coded(
            "MEM_INVALID_SCOPE",
            "workspace binding no longer identifies its recorded canonical path",
        ));
    }
    if !canonical.is_dir() {
        return Err(MemoryError::coded(
            "MEM_INVALID_SCOPE",
            "workspace binding must identify a directory",
        ));
    }
    Ok(canonical)
}

fn matching_project_id(
    projects: &[ProjectRecord],
    key: &str,
) -> Result<Option<String>, MemoryError> {
    let mut id = None;
    for project in projects {
        if workspace_path_key(&project.path) != key {
            continue;
        }
        let scope = MemoryScope::new(MemoryScopeKind::Project, Some(project.id.clone()));
        scope.validate()?;
        if id.as_ref().is_some_and(|id| id != &project.id) {
            return Err(MemoryError::coded(
                "MEM_INVALID_SCOPE",
                "workspace binding has ambiguous project identities",
            ));
        }
        id = Some(project.id.clone());
    }
    Ok(id)
}

fn stable_project_id(canonical_key: &str) -> String {
    let mut id = String::with_capacity(64);
    for byte in Sha256::digest(canonical_key.as_bytes()) {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PROTOCOL_VERSION;
    use crate::storage::ThreadSummary;

    fn thread(db: &ProjectionDb, path: Option<&Path>, in_project: bool) -> String {
        let id = Uuid::new_v4().to_string();
        db.replace_thread(
            &ThreadSummary {
                schema_version: PROTOCOL_VERSION,
                id: id.clone(),
                title: "scope test".into(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                archived: false,
                in_project,
                workspace_path: path.map(|path| path.to_string_lossy().into_owned()),
                model_selection: None,
            },
            &[],
        )
        .unwrap();
        id
    }

    #[test]
    fn standalone_unbound_and_missing_threads_never_borrow_a_project() {
        let db = ProjectionDb::memory().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let resolver = MemoryScopeResolver::new(db.clone());
        let standalone = thread(&db, Some(directory.path()), false);
        let unbound = thread(&db, None, true);
        for id in [standalone, unbound, Uuid::new_v4().to_string()] {
            let scopes = resolver.scopes(&id).unwrap();
            assert_eq!(scopes.len(), 2);
            assert_eq!(scopes[0].kind, MemoryScopeKind::Thread);
            assert_eq!(scopes[0].id.as_deref(), Some(id.as_str()));
            assert_eq!(scopes[1], MemoryScope::user());
            assert_eq!(
                resolver.default_scope(&id, MemoryType::Preference).unwrap(),
                scopes[0]
            );
        }
        assert!(db.list_projects().unwrap().is_empty());
        assert!(resolver.scopes("not-a-uuid").is_err());
    }

    #[test]
    fn existing_project_identity_is_shared_by_project_and_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let db = ProjectionDb::memory().unwrap();
        let project = ProjectRecord {
            id: Uuid::new_v4().to_string(),
            name: "registered".into(),
            path: directory.path().to_string_lossy().into_owned(),
            trusted: true,
            last_opened_at_ms: 1,
        };
        db.upsert_project(&project).unwrap();
        let id = thread(&db, Some(&directory.path().canonicalize().unwrap()), true);
        let resolver = MemoryScopeResolver::new(db.clone());
        let scopes = resolver.scopes(&id).unwrap();
        assert_eq!(scopes.len(), 4);
        assert_eq!(scopes[1].kind, MemoryScopeKind::Project);
        assert_eq!(scopes[2].kind, MemoryScopeKind::Workspace);
        assert_eq!(scopes[1].id.as_ref(), Some(&project.id));
        assert_eq!(scopes[1].id, scopes[2].id);
        assert_eq!(
            resolver.default_scope(&id, MemoryType::WorkState).unwrap(),
            scopes[0]
        );
        assert_eq!(
            resolver.default_scope(&id, MemoryType::Fact).unwrap(),
            scopes[1]
        );
        assert_eq!(db.list_projects().unwrap(), vec![project]);
    }

    #[test]
    fn missing_projects_are_stable_persisted_and_visible_with_the_same_ui_identity() {
        let directory = tempfile::tempdir().unwrap();
        let db = ProjectionDb::memory().unwrap();
        let first = thread(&db, Some(directory.path()), true);
        let second = thread(&db, Some(directory.path()), true);
        let resolver = MemoryScopeResolver::new(db.clone());
        let first_scopes = resolver.scopes(&first).unwrap();
        let second_scopes = resolver.scopes(&second).unwrap();
        assert_eq!(first_scopes[1], second_scopes[1]);
        let canonical = directory.path().canonicalize().unwrap();
        let key = workspace_path_key(canonical.to_str().unwrap());
        assert_eq!(first_scopes[1].id, Some(stable_project_id(&key)));
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(first_scopes[1].id.as_ref(), Some(&projects[0].id));
        assert!(!projects[0].trusted);
        let ui = crate::workbench::register_project(&db, directory.path(), false).unwrap();
        assert_eq!(first_scopes[1].id, Some(ui.id));
        let options = resolver.options(&first).unwrap();
        assert_eq!(options.len(), 4);
        assert_eq!(options[0].scope, "user");
        assert_eq!(options[1].scope, format!("thread:{first}"));
        assert_eq!(options[2].scope, first_scopes[1].canonical());
        assert_eq!(options[3].scope, first_scopes[2].canonical());
        assert!(options.iter().all(|option| !option.label.contains(&key)));
    }

    #[test]
    fn invalid_bindings_fail_closed_without_disclosing_paths() {
        let db = ProjectionDb::memory().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let resolver = MemoryScopeResolver::new(db.clone());
        for path in [
            PathBuf::from("relative-workspace"),
            directory.path().join("missing-workspace"),
            directory.path().join(".."),
        ] {
            let id = thread(&db, Some(&path), true);
            let scopes = resolver.scopes(&id).unwrap();
            assert_eq!(scopes, thread_user_scopes(&id));
            assert_eq!(resolver.options(&id).unwrap().len(), 2);
            assert_eq!(
                resolver.default_scope(&id, MemoryType::WorkState).unwrap(),
                scopes[0]
            );
            let error = resolver.default_scope(&id, MemoryType::Fact).unwrap_err();
            assert_eq!(error.code(), "MEM_INVALID_SCOPE");
            assert!(!error.to_string().contains(path.to_str().unwrap()));
        }
        assert!(db.list_projects().unwrap().is_empty());
    }

    #[test]
    fn binding_must_be_a_directory_and_cannot_borrow_registry_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap();
        let db = ProjectionDb::memory().unwrap();
        let resolver = MemoryScopeResolver::new(db.clone());
        let file = canonical.join("file");
        std::fs::write(&file, "").unwrap();
        let id = thread(&db, Some(&file), true);
        assert_eq!(resolver.scopes(&id).unwrap(), thread_user_scopes(&id));
        assert!(resolver.default_scope(&id, MemoryType::Fact).is_err());

        let child = canonical.join("child");
        std::fs::create_dir(&child).unwrap();
        let alias_id = Uuid::new_v4().to_string();
        db.upsert_project(&ProjectRecord {
            id: alias_id.clone(),
            name: "non-canonical alias".into(),
            path: child.join("..").to_string_lossy().into_owned(),
            trusted: false,
            last_opened_at_ms: 1,
        })
        .unwrap();
        let id = thread(&db, Some(&canonical), true);
        let scope = resolver.default_scope(&id, MemoryType::Fact).unwrap();
        assert_ne!(scope.id.as_deref(), Some(alias_id.as_str()));
    }

    #[test]
    fn management_scopes_require_host_records_and_live_immutable_bindings() {
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap();
        let db = ProjectionDb::memory().unwrap();
        let id = thread(&db, Some(&canonical), true);
        let resolver = MemoryScopeResolver::new(db.clone());
        let scopes = resolver.scopes(&id).unwrap();
        for scope in &scopes {
            resolver.validate_management_scope(scope).unwrap();
        }
        for kind in [
            MemoryScopeKind::Thread,
            MemoryScopeKind::Project,
            MemoryScopeKind::Workspace,
        ] {
            let forged = MemoryScope::new(kind, Some(Uuid::new_v4().to_string()));
            assert!(resolver.validate_management_scope(&forged).is_err());
        }
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE threads SET workspace_path=?1 WHERE id=?2",
                rusqlite::params![canonical.join("missing").to_string_lossy().to_string(), id],
            )?;
            Ok(())
        })
        .unwrap();
        resolver.validate_management_scope(&scopes[0]).unwrap();
        resolver
            .validate_management_scope(&MemoryScope::user())
            .unwrap();
        assert!(resolver.validate_management_scope(&scopes[1]).is_err());
        assert!(resolver.validate_management_scope(&scopes[2]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn retargeted_binding_does_not_follow_a_symlink_into_another_project() {
        let directory = tempfile::tempdir().unwrap();
        let bound = directory.path().join("bound");
        let other = directory.path().join("other");
        std::fs::create_dir(&bound).unwrap();
        std::fs::create_dir(&other).unwrap();
        let db = ProjectionDb::memory().unwrap();
        let id = thread(&db, Some(&bound.canonicalize().unwrap()), true);
        std::fs::remove_dir(&bound).unwrap();
        std::os::unix::fs::symlink(&other, &bound).unwrap();
        let resolver = MemoryScopeResolver::new(db);
        assert_eq!(resolver.scopes(&id).unwrap(), thread_user_scopes(&id));
        assert!(resolver.default_scope(&id, MemoryType::Fact).is_err());
    }
}
