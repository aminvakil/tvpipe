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
        Command::Pair { target, pin } => crate::airplay::pair::pair(&target, pin.as_deref()),
        Command::Unpair { target } => crate::airplay::pair::unpair(&target),
        Command::Play(opts) | Command::TestPattern(opts) => play(opts),
        Command::DebugPipeline(o) => crate::media::debug::run(&o),
    }
}

fn play(_opts: PlayOptions) -> Result<()> {
    Err(Error::unimplemented("play (M6/M8)"))
}
