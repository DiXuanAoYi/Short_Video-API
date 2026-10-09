//! 剪辑：多段素材（视频、图片）在时间线上排布，加转场、文字、配乐，导出成一个视频。
//!
//! - [`spec`]：工程（片段、文字、音频轨、输出规格）和时间线排布计算
//! - [`fonts`]：给文字找默认字体
//! - [`build`]：工程 → ffmpeg 参数（纯函数）
//! - [`job`]：后台任务（导出、生成预览）和界面用的命令

pub mod build;
pub mod cmds;
pub mod fonts;
pub mod job;
pub mod spec;

#[cfg(test)]
mod e2e_tests;
