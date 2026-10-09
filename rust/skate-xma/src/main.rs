//! skate-xma CLI (LGPL-2.1-or-later, see lib.rs).
//!
//!   skate-xma --version
//!   skate-xma --continuous FILE JOBS OUTDIR
//!       Drop-in for `xmadec --continuous`: one line per sound in JOBS,
//!       `NAME DATA_OFFSET SAMPLES CHANNELS RATE`; writes OUTDIR/NAME.xma16
//!       (big-endian PCM16, interleaved if stereo, whole 512-sample frames
//!       from the first). One decoder context per run, carried across jobs
//!       as in xmadec.
//!   skate-xma --snr FILE [DATA_OFFSET] OUT.xma16
//!       Standalone EA SNR stream whose header is at DATA_OFFSET (default 0;
//!       for a .grain file pass the u32 BE stored at byte 0).

use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: skate-xma --version\n       skate-xma --continuous FILE JOBS OUTDIR  (job: NAME OFFSET SAMPLES CHANNELS RATE)\n       skate-xma --snr FILE [DATA_OFFSET] OUT.xma16"
    );
    ExitCode::from(2)
}

fn run_jobs(file: &str, jobs: &str, outdir: &str) -> Result<(), String> {
    let data = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    let jobs = std::fs::read_to_string(jobs).map_err(|e| format!("{jobs}: {e}"))?;
    let mut dec = skate_xma::Decoder::new();
    let mut done = 0usize;
    for line in jobs.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(off), Some(samples), Some(ch), Some(rate)) = (it.next(), it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        let (Ok(off), Ok(samples), Ok(ch), Ok(_rate)) =
            (off.parse::<usize>(), samples.parse::<u32>(), ch.parse::<u32>(), rate.parse::<u32>())
        else {
            continue;
        };
        if ch > 2 {
            eprintln!("{name}: {ch} channels (multi-stream) not handled");
            continue;
        }
        let pcm = dec.decode_sample(&data, off, samples, ch).map_err(|e| format!("{name}: {e}"))?;
        let path = format!("{outdir}/{name}.xma16");
        std::fs::write(&path, pcm.to_be_bytes()).map_err(|e| format!("{path}: {e}"))?;
        if pcm.av_errors != 0 {
            eprintln!("{name}: {} FFmpeg errors", pcm.av_errors);
        }
        done += 1;
    }
    println!("{done} sounds");
    Ok(())
}

fn run_snr(file: &str, offset: usize, out: &str) -> Result<(), String> {
    let data = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
    if offset >= data.len() {
        return Err(format!("{file}: offset {offset} past end ({} bytes)", data.len()));
    }
    let (h, pcm) = skate_xma::decode_snr(&data[offset..]).map_err(|e| format!("{file}: {e}"))?;
    std::fs::write(out, pcm.to_be_bytes()).map_err(|e| format!("{out}: {e}"))?;
    println!(
        "{file}: {} ch, {} Hz (header {} Hz), {} frames = {} samples, header samples {}{}",
        pcm.channels,
        pcm.sample_rate,
        h.sample_rate,
        pcm.frames,
        pcm.frames * skate_xma::SAMPLES_PER_FRAME,
        h.samples,
        if pcm.av_errors != 0 { format!(", {} FFmpeg errors", pcm.av_errors) } else { String::new() }
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut continuous = false;
    let mut snr = false;
    let mut rest: Vec<&str> = Vec::new();
    for a in &args {
        match a.as_str() {
            "--version" | "-V" => {
                println!("{}", skate_xma::VERSION_LINE);
                return ExitCode::SUCCESS;
            }
            "--continuous" => continuous = true,
            "--snr" => snr = true,
            s if s.starts_with("--") => return usage(),
            s => rest.push(s),
        }
    }
    let r = if snr {
        match rest.as_slice() {
            [file, out] => run_snr(file, 0, out),
            [file, off, out] => match off.parse::<usize>() {
                Ok(o) => run_snr(file, o, out),
                Err(_) => return usage(),
            },
            _ => return usage(),
        }
    } else if continuous {
        match rest.as_slice() {
            [file, jobs, outdir] => run_jobs(file, jobs, outdir),
            _ => return usage(),
        }
    } else {
        eprintln!("skate-xma: only --continuous and --snr decoding are implemented");
        return usage();
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("skate-xma: {e}");
            ExitCode::FAILURE
        }
    }
}
