//! Questions to the person at the terminal, on stderr.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{Context, Result, bail};

/// A yes/no question; only `y` or `yes` is yes, and the end of input is no.
pub fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    if std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("reading the answer")?
        == 0
    {
        eprintln!();
        return Ok(false);
    }
    let a = line.trim().to_ascii_lowercase();
    Ok(a == "y" || a == "yes")
}

/// One line, not echoed when stdin is a terminal.
pub fn secret(prompt: &str) -> Result<String> {
    eprint!("{prompt}: ");
    std::io::stderr().flush()?;
    let stdin = std::io::stdin();
    let saved = if stdin.is_terminal() {
        Some(echo_off()?)
    } else {
        None
    };
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if let Some(t) = saved {
        restore(&t);
        eprintln!();
    }
    if read.context("reading the secret")? == 0 {
        bail!("no input for {prompt}");
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn echo_off() -> Result<libc::termios> {
    // SAFETY: termios is plain data that tcgetattr fills for fd 0.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut saved) } != 0 {
        bail!("cannot read the terminal's settings");
    }
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    // SAFETY: quiet is a valid termios copied from the terminal's own.
    if unsafe { libc::tcsetattr(0, libc::TCSANOW, &quiet) } != 0 {
        bail!("cannot turn off the terminal's echo");
    }
    Ok(saved)
}

fn restore(saved: &libc::termios) {
    // SAFETY: saved came from tcgetattr on fd 0.
    unsafe { libc::tcsetattr(0, libc::TCSANOW, saved) };
}
