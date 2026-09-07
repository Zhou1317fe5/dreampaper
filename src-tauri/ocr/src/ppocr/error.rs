use thiserror::Error;

#[derive(Error, Debug)]
pub enum OcrError {
    #[error("ONNX Runtime 错误")]
    Ort(#[from] ort::Error),
    #[error("读取字典失败")]
    Io(#[from] std::io::Error),
    #[error("模型 Session 未初始化")]
    SessionNotInitialized,
    #[error("模型输出无效：{0}")]
    ModelOutput(String),
}
