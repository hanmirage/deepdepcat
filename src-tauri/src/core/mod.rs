//! Core infrastructure for DeepDepCat.
//!
//! This module provides the foundational types, error handling, configuration
//! management, and global state that all other subsystems build upon.

pub mod config;
pub mod crash;
pub mod dsml;
pub mod encoding;
pub mod error;
pub mod feature_flag;
pub mod ids;
pub mod image_codec;
pub mod image_codec_validate;
pub mod managed;
pub mod pattern;
pub mod proc;
pub mod str_util;
pub mod stream;
pub mod types;
