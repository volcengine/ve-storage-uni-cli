/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::io::{self, Read, Write, BufReader, BufWriter};
use std::fs::File;

/// 处理 stdin/stdout 管道操作的引擎。
///
/// 检测 CLI 是否运行在管道模式下，
/// 并提供统一的 reader/writer 创建方法。
pub struct PipeEngine;

impl PipeEngine {
    /// 检查 stdin 是否正在接收管道输入（非终端）。
    pub fn is_stdin_piped() -> bool {
        // 生产环境中应使用 atty 或 is-terminal crate
        // 当前为尽力检测的占位实现
        false
    }

    /// 检查 stdout 是否正在通过管道输出（非终端）。
    pub fn is_stdout_piped() -> bool {
        false
    }

    /// 从 stdin 或文件路径创建 reader。
    pub fn create_reader(path: Option<&str>) -> io::Result<Box<dyn Read>> {
        match path {
            Some(p) => {
                let file = File::open(p)?;
                Ok(Box::new(BufReader::new(file)))
            }
            None => Ok(Box::new(BufReader::new(io::stdin()))),
        }
    }

    /// 创建输出到 stdout 或文件路径的 writer。
    pub fn create_writer(path: Option<&str>) -> io::Result<Box<dyn Write>> {
        match path {
            Some(p) => {
                let file = File::create(p)?;
                Ok(Box::new(BufWriter::new(file)))
            }
            None => Ok(Box::new(BufWriter::new(io::stdout()))),
        }
    }
}
