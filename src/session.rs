//! Orchestration: connect → plan → ffmpeg → fanout → teardown (PLAN §3).
//!
//! Owner: lead.

use crate::cli::{Command, PlayOptions};
use crate::error::{Error, Result};

pub fn run(cmd: Command) -> Result<()> {
    match cmd {
        Command::Discover {
            timeout,
            json,
            interfaces,
        } => crate::net::discover::run(timeout, json, &interfaces),
        Command::Pair { .. } => Err(Error::unimplemented("pair (M4)")),
        Command::Unpair { .. } => Err(Error::unimplemented("unpair (M4)")),
        Command::Play(opts) | Command::TestPattern(opts) => play(opts),
        Command::DebugPipeline(_) => Err(Error::unimplemented("debug-pipeline (M3)")),
    }
}

fn play(_opts: PlayOptions) -> Result<()> {
    Err(Error::unimplemented("play (M6/M8)"))
}
