// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! A console program that drives a BonDriver the way a host does.
//!
//! It exists because the hosts are windowed applications: over a remote shell there is no
//! desktop to show them on, and a scan that fails leaves a dialog nobody can dismiss. This
//! asks the same questions in the same order and prints the answers, so the parts most
//! likely to be wrong fail here, where the failure is readable.
//!
//! ```text
//! bondriver-probe BonDriver_coax.dll              enumerate and stop
//! bondriver-probe BonDriver_coax.dll 0 3          tune space 0, channel 3
//! bondriver-probe BonDriver_coax.dll 0 3 out.ts   and write what arrives
//! ```

//! Windows only: it loads a Windows library through the Windows loader. On
//! anything else it says so and stops, so that a workspace-wide `cargo test`
//! still runs.
#![allow(non_snake_case)]

#[cfg(not(windows))]
fn main() {
    eprintln!("bondriver-probe drives a Windows library; build it for a Windows target");
}

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::ffi::{OsStr, c_void};
#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::io::Write;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::process::ExitCode;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
mod bon;
#[cfg(windows)]
mod library;

#[cfg(windows)]
use bon::Bon;
#[cfg(windows)]
use library::Library;

/// How long to hold a channel when the caller asked for the stream.
#[cfg(windows)]
const STREAM_FOR: Duration = Duration::from_secs(10);

/// How long to read each channel for, in seconds: `PROBE_SECONDS` in the
/// environment, or ten.
#[cfg(windows)]
fn stream_for() -> Duration {
    env::var("PROBE_SECONDS")
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .map_or(STREAM_FOR, Duration::from_secs)
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleOutputCP(code_page: u32) -> i32;
}

/// The console this is read through is a remote shell, whose code page is not
/// the one Rust prints in. Without this the station names come back as rubble.
#[cfg(windows)]
fn use_utf8_console() {
    const UTF8: u32 = 65001;
    unsafe { SetConsoleOutputCP(UTF8) };
}

#[cfg(windows)]
fn main() -> ExitCode {
    use_utf8_console();
    let arguments: Vec<String> = env::args().skip(1).collect();
    let Some(path) = arguments.first() else {
        eprintln!("usage: bondriver-probe <BonDriver.dll> [space channel [out.ts]]");
        return ExitCode::FAILURE;
    };
    match run(path, &arguments[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("bondriver-probe: {reason}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn run(path: &str, rest: &[String]) -> Result<(), String> {
    let library = Library::open(Path::new(path))?;
    let create = library.create_bon_driver()?;
    let bon = Bon::create(create).ok_or("CreateBonDriver answered with nothing")?;

    println!("tuner: {}", bon.tuner_name());
    enumerate(&bon);

    let (Some(space), Some(channel)) = (rest.first(), rest.get(1)) else {
        return Ok(());
    };
    let space: u32 = space
        .parse()
        .map_err(|_| format!("space {space} is not a number"))?;
    // Several channels, comma separated, are tuned one after another on the
    // one open tuner, which is what a host's channel scan does.
    let channels = channel
        .split(',')
        .map(|one| {
            one.parse::<u32>()
                .map_err(|_| format!("channel {one} is not a number"))
        })
        .collect::<Result<Vec<u32>, String>>()?;

    if !bon.open_tuner() {
        return Err("OpenTuner refused".to_owned());
    }
    println!("opened, tuner reports open = {}", bon.is_tuner_opening());

    for (at, channel) in channels.iter().enumerate() {
        if !bon.set_channel(space, *channel) {
            return Err(format!("SetChannel({space}, {channel}) refused"));
        }
        println!(
            "tuned to space {} channel {} (driver says {}/{}), signal {:.1}",
            space,
            channel,
            bon.current_space(),
            bon.current_channel(),
            bon.signal_level()
        );

        match rest.get(2) {
            Some(output) => {
                let output = match channels.len() {
                    1 => output.clone(),
                    _ => format!("{output}.{at}"),
                };
                stream(&bon, &output)?;
                // Asked again at the end: the driver has the daemon measure
                // while the stream runs, so the first answer is from before
                // there was anything to measure.
                println!("signal after reading: {:.1}", bon.signal_level());
            }
            None => println!("no output file given, so the stream was not read"),
        }
    }
    Ok(())
}

/// Walks the spaces and channels exactly as a host does: from zero, until a
/// null says there are no more. A driver whose list is not dense loses
/// everything after its first gap, and that shows up here as a short list.
#[cfg(windows)]
fn enumerate(bon: &Bon) {
    for space in 0.. {
        let Some(name) = bon.tuning_space_name(space) else {
            println!("{space} spaces");
            break;
        };
        println!("space {space}: {name}");
        for channel in 0.. {
            let Some(channel_name) = bon.channel_name(space, channel) else {
                println!("  {channel} channels");
                break;
            };
            println!("  {channel}: {channel_name}");
        }
    }
}

/// Reads for a while and writes what arrives, reporting as it goes.
#[cfg(windows)]
fn stream(bon: &Bon, output: &str) -> Result<(), String> {
    let mut file = File::create(output).map_err(|reason| reason.to_string())?;
    let started = Instant::now();
    let mut written = 0usize;
    let mut waits = 0u32;
    let stream_for = stream_for();
    while started.elapsed() < stream_for {
        // A driver answers this with zero when something is ready and with a
        // timeout code when nothing is, which is the whole of its flow control.
        if bon.wait_ts_stream(200) != 0 {
            waits += 1;
            continue;
        }
        let Some(bytes) = bon.take_ts_stream() else {
            waits += 1;
            continue;
        };
        file.write_all(bytes).map_err(|reason| reason.to_string())?;
        written += bytes.len();
    }
    println!(
        "read {written} bytes in {:.1}s ({waits} waits) into {output}",
        started.elapsed().as_secs_f64()
    );
    if written == 0 {
        return Err("the driver locked but carried nothing".to_owned());
    }
    Ok(())
}

/// Windows takes its paths and its strings as UTF-16.
#[cfg(windows)]
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// The signature of the one function a BonDriver exports.
#[cfg(windows)]
type CreateBonDriver = unsafe extern "C" fn() -> *mut c_void;
