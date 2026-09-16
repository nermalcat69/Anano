//! git2 调用失败的分类：区分「打开仓库失败」与「读取状态失败」，界面文案各给一句。

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("couldn't open repository: {0}")]
    Open(#[source] git2::Error),
    #[error("couldn't read repository status: {0}")]
    Status(#[source] git2::Error),
    #[error("couldn't create commit: {0}")]
    Commit(#[source] git2::Error),
    #[error("branch operation failed: {0}")]
    Branch(#[source] git2::Error),
    #[error("remote operation failed: {0}")]
    Remote(#[source] git2::Error),
    /// `pull` 只做快进；本地与上游各自有新提交时不自动合并，交给用户自己处理。
    #[error("local branch has diverged from its upstream and needs a manual merge")]
    Diverged,
    #[error("current branch has no upstream configured")]
    NoUpstream,
    #[error("git-lfs failed: {0}")]
    Lfs(String),
    #[error("couldn't discard change: {0}")]
    Discard(String),
    #[error("couldn't update .gitignore: {0}")]
    Ignore(String),
    #[error("couldn't clone repository: {0}")]
    Clone(String),
    #[error("couldn't create repository: {0}")]
    Init(String),
}
