#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("invalid proof: {0}")]
    InvalidProof(String),
    #[error("bundle parse error: {0}")]
    BundleParse(String),
    #[error("verification failed: {0}")]
    VerificationFailed(String),
}
pub type Result<T> = std::result::Result<T, VerifyError>;
