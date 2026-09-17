//! 项目归属的服务端解析。
//!
//! 背景：项目归属事实原本分叉在两处——一半在 `projects` 表与会话的
//! `workspace_path` 列（服务端），另一半在桌面端的 `localStorage`（已知项目列表、
//! 会话到项目的展示映射）。后果是任何非桌面端的消费者（手机网关就是第一个）都只能
//! 看到"已经有会话挂在某个路径下"的项目，而**用户显式添加过、但还没在项目下建会话**
//! 的项目在服务端根本不可见。
//!
//! 本模块把那条事实收回到服务端：
//! - 项目清单以 `projects` 表为唯一来源，去重后按 `last_opened_at_ms` 倒序；
//! - 会话到项目的归属由服务端解析成 [`workspace_path_key`]，消费者不再自行推断。
//!
//! [`workspace_path_key`]: crate::workbench::workspace_path_key

use std::collections::HashMap;

use crate::app_state::AppState;
use crate::persistence::ProjectRecord;
use crate::storage::{StorageError, ThreadSummary};
use crate::workbench::workspace_path_key;

/// 服务端解析出的项目归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRef {
    /// `projects` 表主键。
    pub id: String,
    pub name: String,
    /// 归属键，见 [`workspace_path_key`]。
    pub key: String,
    pub last_opened_at_ms: u64,
}

impl ProjectRef {
    fn from_record(record: &ProjectRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            key: workspace_path_key(&record.path),
            last_opened_at_ms: record.last_opened_at_ms,
        }
    }
}

/// 项目清单 + 会话归属的一次性快照。
#[derive(Debug, Clone)]
pub struct ProjectAttribution {
    /// 去重后的项目清单，按 `last_opened_at_ms` 倒序。
    pub projects: Vec<ProjectRef>,
    /// 会话 ID → 项目归属键。不含独立会话与无法归属的会话。
    pub thread_projects: HashMap<String, String>,
}

impl ProjectAttribution {
    /// 某个会话的归属键。
    pub fn project_key_of(&self, thread_id: &str) -> Option<String> {
        self.thread_projects.get(thread_id).cloned()
    }
}

/// 读取项目清单并解析会话归属。
///
/// `active_workspace` 是当前活动工作区（`AppState::workspace_root`）。它的作用是给
/// **`in_project = true` 但 `workspace_path` 仍为 NULL 的历史会话**兜底：那些会话在
/// 归属列出现之前就已存在，桌面端靠 `localStorage` 的展示映射把它们归到当前工作区；
/// 服务端没有那份映射，只能用"当前活动工作区"这个唯一可得的事实补上，否则它们在手机
/// 上会既不属于任何项目、又因为 `in_project = true` 而不落进独立会话区，成为幽灵。
pub async fn resolve(
    state: &AppState,
    active_workspace: Option<&str>,
) -> Result<ProjectAttribution, StorageError> {
    let records = state
        .repository()
        .projection()
        .list_projects()
        .map_err(|error| StorageError::Io(error.to_string()))?;

    let projects = dedupe_projects(records);

    let threads = state.list_conversation_threads("").await?;
    let mut thread_projects = HashMap::new();
    for thread in &threads {
        if let Some(key) = resolve_thread_project(thread, active_workspace) {
            thread_projects.insert(thread.id.clone(), key);
        }
    }

    Ok(ProjectAttribution {
        projects,
        thread_projects,
    })
}

/// 把 `projects` 表按归属键去重。
///
/// 同一个目录可能因为历史原因留下多行（例如路径写法不同，或曾被删除后重新添加）。
/// 取 `last_opened_at_ms` 最新的那一行：用户最近一次打开的项目名才是他想看到的。
fn dedupe_projects(records: Vec<ProjectRecord>) -> Vec<ProjectRef> {
    let mut by_key: HashMap<String, ProjectRef> = HashMap::new();
    for record in &records {
        let candidate = ProjectRef::from_record(record);
        by_key
            .entry(candidate.key.clone())
            .and_modify(|existing| {
                if candidate.last_opened_at_ms > existing.last_opened_at_ms {
                    *existing = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    let mut projects: Vec<ProjectRef> = by_key.into_values().collect();
    projects.sort_by(|left, right| {
        right
            .last_opened_at_ms
            .cmp(&left.last_opened_at_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    projects
}

/// 解析单个会话的项目归属键。
///
/// 三种情况：
/// - 显式独立会话（`in_project = false`）→ `None`，它不该出现在任何项目分组里；
/// - 已绑定工作区 → 用绑定路径算键；
/// - 未绑定但属于项目的遗留会话 → 落到 `active_workspace`（见 [`resolve`] 的说明）。
fn resolve_thread_project(
    thread: &ThreadSummary,
    active_workspace: Option<&str>,
) -> Option<String> {
    if !thread.in_project {
        return None;
    }
    if let Some(bound) = thread.workspace_path.as_deref() {
        // 绑定的工作区可能已从项目列表中移除（用户删掉了分组但会话还在）。
        // 仍按真实归属返回，让手机端能把它显示出来而不是静默丢弃——
        // 手机端认不出这个键时会渲染成「（已移除）」的分组。
        return Some(workspace_path_key(bound));
    }
    // 遗留会话：归属列出现之前就存在，桌面端靠 localStorage 的展示映射把它们归到
    // 当前工作区。服务端没有那份映射，只能用「当前活动工作区」这个唯一可得的事实补上，
    // 否则它们在手机上既不属于任何项目、又因为 `in_project = true` 而不落进独立会话区。
    // 活动工作区通常已被 `workspace_state` 注册进项目表，因此这个键一般是已知的；
    // 即便不是也照常返回，它至少是这批会话共同的事实来源。
    Some(workspace_path_key(active_workspace?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PROTOCOL_VERSION;

    fn thread(id: &str, in_project: bool, workspace_path: Option<&str>) -> ThreadSummary {
        ThreadSummary {
            schema_version: PROTOCOL_VERSION,
            id: id.to_string(),
            title: id.to_string(),
            created_at_ms: 1,
            updated_at_ms: 1,
            archived: false,
            in_project,
            workspace_path: workspace_path.map(str::to_string),
        }
    }

    fn record(id: &str, name: &str, path: &str, last_opened_at_ms: u64) -> ProjectRecord {
        ProjectRecord {
            id: id.to_string(),
            name: name.to_string(),
            path: path.to_string(),
            trusted: true,
            last_opened_at_ms,
        }
    }

    #[test]
    fn dedupe_keeps_the_most_recently_opened_row_per_key() {
        let projects = dedupe_projects(vec![
            record("stale", "old-name", r"D:\code\app", 10),
            record("fresh", "app", r"D:\code\app", 99),
        ]);

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "fresh");
        assert_eq!(projects[0].name, "app");
    }

    #[test]
    fn dedupe_sorts_newest_first_and_breaks_ties_by_name() {
        let projects = dedupe_projects(vec![
            record("b", "beta", r"D:\code\b", 5),
            record("a", "alpha", r"D:\code\a", 5),
            record("c", "gamma", r"D:\code\c", 50),
        ]);

        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["gamma", "alpha", "beta"]);
    }

    /// 归属键必须把 `\\?\` 前缀、分隔符和大小写折叠掉，否则 `projects` 表里
    /// 由 `canonicalize` 写入的路径和会话里历史写入的路径会算出两个键。
    #[test]
    fn path_key_folds_prefix_separators_and_case() {
        assert_eq!(
            workspace_path_key(r"\\?\D:\code\Nick\k-coder"),
            workspace_path_key("D:/code/Nick/k-coder")
        );
        if cfg!(windows) {
            assert_eq!(
                workspace_path_key(r"D:\Code\APP"),
                workspace_path_key("d:/code/app")
            );
        }
    }

    /// 同一个项目既出现在 `projects` 表（带前缀）又出现在会话绑定里（不带前缀）时，
    /// 会话必须归到项目清单里的那个键上，而不是自成一个分组。
    #[test]
    fn thread_bound_with_a_different_path_spelling_lands_on_the_project_key() {
        let projects =
            dedupe_projects(vec![record("p", "k-coder", r"\\?\D:\code\Nick\k-coder", 1)]);

        let key = resolve_thread_project(&thread("t", true, Some(r"D:\code\Nick\k-coder")), None);

        assert_eq!(key.as_deref(), Some(projects[0].key.as_str()));
    }

    /// 独立会话永远不属于任何项目。
    #[test]
    fn standalone_threads_are_never_attributed() {
        assert!(resolve_thread_project(&thread("t", false, Some(r"D:\code\app")), None).is_none());
        assert!(resolve_thread_project(&thread("t", false, None), None).is_none());
    }

    /// 遗留会话（`in_project = true`、`workspace_path = NULL`）兜底到当前活动工作区，
    /// 否则它们既不在项目分组里、又不属于独立会话，在手机上会彻底消失。
    #[test]
    fn legacy_threads_fall_back_to_the_active_workspace() {
        let key = resolve_thread_project(&thread("t", true, None), Some(r"D:\code\Nick\k-coder"));

        assert_eq!(
            key.as_deref(),
            Some(workspace_path_key(r"D:\code\Nick\k-coder").as_str())
        );
    }

    /// 没有活动工作区可用时，宁可不成组也不要编造归属。
    #[test]
    fn legacy_threads_without_an_active_workspace_stay_unattributed() {
        assert!(resolve_thread_project(&thread("t", true, None), None).is_none());
    }

    /// 绑定路径的会话即使指向一个已移除的项目，也要保留归属键——
    /// 手机端会把它渲染成「（已移除）」分组，而不是让会话凭空消失。
    #[test]
    fn threads_bound_to_a_removed_project_keep_their_key() {
        let key = resolve_thread_project(&thread("t", true, Some(r"D:\code\gone")), None);

        assert_eq!(
            key.as_deref(),
            Some(workspace_path_key(r"D:\code\gone").as_str())
        );
    }

    /// `resolve()` 的完整链路：真实 `ProjectionDb` → 项目清单 → 归属解析。
    ///
    /// 这条断言钉住的正是本次修复的核心主张——**一个还没有任何会话的项目也必须出现
    /// 在清单里**。「有项目、零会话」本身就是有效信息，而不是「这个项目不存在」；
    /// 修复前手机端唯一的入口 `thread/list` 只投影会话表，这类项目会整个消失。
    #[tokio::test]
    async fn resolve_returns_projects_that_have_no_sessions_at_all() {
        let data = tempfile::tempdir().unwrap();
        let builtin = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::new_with_builtin_skills(data.path(), builtin.path()).unwrap();

        let empty_project = workspace.path().join("empty-project");
        std::fs::create_dir_all(&empty_project).unwrap();
        crate::workbench::register_project(&state.repository().projection(), &empty_project, true)
            .unwrap();

        let attribution = resolve(&state, None).await.unwrap();

        assert_eq!(attribution.projects.len(), 1, "零会话项目必须出现在清单里");
        assert_eq!(attribution.projects[0].name, "empty-project");
        assert_eq!(
            attribution.projects[0].key,
            workspace_path_key(&empty_project.to_string_lossy())
        );
        assert!(
            attribution.thread_projects.is_empty(),
            "没有会话就不该解析出任何归属"
        );
    }

    /// 同一个目录在 `projects` 表里留下多行（历史路径写法不同）时，
    /// `resolve()` 必须只下发一行——否则手机端会渲染出重复的空分组。
    #[tokio::test]
    async fn resolve_dedupes_duplicate_rows_for_the_same_directory() {
        let data = tempfile::tempdir().unwrap();
        let builtin = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let state = AppState::new_with_builtin_skills(data.path(), builtin.path()).unwrap();

        let project = workspace.path().join("dup");
        std::fs::create_dir_all(&project).unwrap();
        let canonical = project.canonicalize().unwrap();
        let projection = state.repository().projection();
        crate::workbench::register_project(&projection, &canonical, true).unwrap();
        // 再插一行同目录、但路径写法不同的记录（模拟历史遗留）。
        //
        // 写法差异只补一个**尾分隔符**，且保持原有分隔符风格：`workspace_path_key`
        // 是在 `\`→`/` 之前按字面反斜杠剥离 `\\?\` 前缀的，若提前把反斜杠换成 `/`，
        // 前缀就剥不掉、键会变成 `//?/d:/...`，反而构不成"同键不同写法"。
        let mut stale = projection.list_projects().unwrap().remove(0);
        stale.id = "stale".to_string();
        stale.name = "stale-name".to_string();
        stale.last_opened_at_ms = stale.last_opened_at_ms.saturating_sub(1_000);
        stale.path = format!(
            "{}\\",
            stale.path.trim_end_matches(|c| c == '\\' || c == '/')
        );
        assert_eq!(
            workspace_path_key(&stale.path),
            workspace_path_key(&canonical.to_string_lossy()),
            "构造出来的写法差异必须归一化到同一个键，否则这个用例测不到去重"
        );
        projection.upsert_project(&stale).unwrap();

        let attribution = resolve(&state, None).await.unwrap();

        assert_eq!(attribution.projects.len(), 1, "同目录只应下发一行");
        assert_ne!(
            attribution.projects[0].name, "stale-name",
            "应当保留 last_opened_at_ms 更新的那一行"
        );
    }
}
