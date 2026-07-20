use shell_quote::Sh;
use std::io::{self, BufReader, Read, Write};
use std::{error::Error, process::Command};

/// Quote a string for safe use in POSIX shell commands.
pub fn shell_quote(s: &str) -> String {
  // Sh::quote_vec always produces ASCII, so this is safe.
  String::from_utf8(Sh::quote_vec(s)).unwrap()
}

/// Run a command on the remote via SSH, streaming output to the local terminal.
///
/// Allocates a pseudo-terminal (`-t`) so remote programs can emit
/// colors. Stderr from the PTY teardown ("Connection closed") is
/// suppressed because it's cosmetic noise.
///
/// SGR (color/style) escape sequences are preserved; all other CSI
/// sequences that leak through the PTY (DSR, DA, cursor movement, etc.)
/// are discarded so they don't appear as garbage.
pub fn ssh_run(host: &str, cmd: &str) -> Result<(), Box<dyn Error>> {
  let mut child = Command::new("ssh")
    .args(["-t", host, cmd])
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null())
    .spawn()?;

  let stdout = child.stdout.take().unwrap();
  filter_sgr(BufReader::new(stdout), io::stdout())?;

  let status = child.wait()?;
  if !status.success() {
    return Err(
      format!(
        "Remote command failed with exit code: {}",
        status.code().unwrap_or(-1)
      )
      .into(),
    );
  }

  Ok(())
}

/// Whitelist CSI escape sequences, allowing only SGR (Select Graphic
/// Rendition) — the sequences that set colors and text styles.
///
/// When SSH is invoked with PTY allocation (`-t`), the remote terminal
/// can respond to queries (cursor position, device attributes, etc.)
/// and those responses leak into stdout as raw escape sequences.  This
/// filter passes through plain text and SGR sequences (`ESC[...m`) so
/// colours are preserved, while discarding every other CSI sequence
/// (DSR `ESC[row;colR`, DA `ESC[?...c`, cursor movement, etc.).
fn filter_sgr(
  mut reader: impl Read,
  mut writer: impl Write,
) -> io::Result<()> {
  const S_NORMAL: u8 = 0;
  const S_ESC: u8 = 1;
  const S_CSI: u8 = 2;

  let mut state = S_NORMAL;
  let mut seq_buf: Vec<u8> = Vec::new();
  let mut byte = [0u8; 1];

  loop {
    match reader.read(&mut byte)? {
      0 => break,
      _ => {}
    }
    let b = byte[0];

    match state {
      S_NORMAL => {
        if b == 0x1b {
          seq_buf.clear();
          seq_buf.push(b);
          state = S_ESC;
        } else {
          writer.write_all(&byte)?;
        }
      }
      S_ESC => {
        if b == b'[' {
          seq_buf.push(b);
          state = S_CSI;
        } else {
          // Non-CSI escape (e.g. ESC7, ESC8) — pass through.
          seq_buf.push(b);
          writer.write_all(&seq_buf)?;
          seq_buf.clear();
          state = S_NORMAL;
        }
      }
      S_CSI => {
        seq_buf.push(b);
        if b.is_ascii_alphabetic() || b == b'~' {
          // Final byte reached — allow only SGR (m).
          if b == b'm' {
            writer.write_all(&seq_buf)?;
          }
          seq_buf.clear();
          state = S_NORMAL;
        }
      }
      _ => unreachable!(),
    }
  }

  if !seq_buf.is_empty() {
    writer.write_all(&seq_buf)?;
  }
  writer.flush()?;
  Ok(())
}

/// Resolve `$HOME` on the remote host.
pub fn resolve_home(host: &str) -> Result<String, Box<dyn Error>> {
  ssh_capture(host, "echo $HOME")
}

/// Run a command on the remote via SSH and capture stdout.
pub fn ssh_capture(
  host: &str,
  cmd: &str,
) -> Result<String, Box<dyn Error>> {
  let output = Command::new("ssh").args([host, cmd]).output()?;

  if !output.status.success() {
    let status = output.status;
    let err = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut msg = format!(
      "Remote command failed with exit code: {}",
      status.code().unwrap_or(-1)
    );
    if !err.is_empty() {
      msg.push_str(&format!("\nstderr: {err}"));
    }
    if !stdout.is_empty() {
      msg.push_str(&format!("\nstdout: {stdout}"));
    }
    return Err(msg.into());
  }

  Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn filter(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    filter_sgr(input, &mut output).unwrap();
    output
  }

  #[test]
  fn plain_text_passthrough() {
    let out = filter(b"hello world");
    assert_eq!(out, b"hello world");
  }

  #[test]
  fn empty_input() {
    let out = filter(b"");
    assert!(out.is_empty());
  }

  #[test]
  fn sgr_color_allowed() {
    let out = filter(b"\x1b[31mred\x1b[0m");
    assert_eq!(out, b"\x1b[31mred\x1b[0m");
  }

  #[test]
  fn sgr_bold_allowed() {
    let out = filter(b"\x1b[1mbold\x1b[22m");
    assert_eq!(out, b"\x1b[1mbold\x1b[22m");
  }

  #[test]
  fn sgr_256_color_allowed() {
    let out = filter(b"\x1b[38;5;196m");
    assert_eq!(out, b"\x1b[38;5;196m");
  }

  #[test]
  fn sgr_24bit_color_allowed() {
    let out = filter(b"\x1b[48;2;255;128;0m");
    assert_eq!(out, b"\x1b[48;2;255;128;0m");
  }

  #[test]
  fn sgr_shorthand_reset_allowed() {
    // ESC[m is a valid shorthand for ESC[0m
    let out = filter(b"\x1b[m");
    assert_eq!(out, b"\x1b[m");
  }

  #[test]
  fn sgr_with_intermediate_byte_allowed() {
    // ESC[ m has an intermediate space — still SGR
    let out = filter(b"\x1b[ m");
    assert_eq!(out, b"\x1b[ m");
  }

  #[test]
  fn dsr_response_stripped() {
    // The original issue: cursor position report
    let out = filter(b"\x1b[21;1R");
    assert!(out.is_empty());
  }

  #[test]
  fn dec_private_dsr_stripped() {
    let out = filter(b"\x1b[?21;1R");
    assert!(out.is_empty());
  }

  #[test]
  fn dec_private_dsr_extended_stripped() {
    // DECXCPR: ESC[?row;col;1R
    let out = filter(b"\x1b[?5;10;1R");
    assert!(out.is_empty());
  }

  #[test]
  fn da_response_stripped() {
    // Device attribute response
    let out = filter(b"\x1b[?1;2c");
    assert!(out.is_empty());
  }

  #[test]
  fn da2_response_stripped() {
    let out = filter(b"\x1b[>62;1;2c");
    assert!(out.is_empty());
  }

  #[test]
  fn cursor_movement_stripped() {
    let out = filter(b"\x1b[2A");
    assert!(out.is_empty());
  }

  #[test]
  fn cursor_home_stripped() {
    let out = filter(b"\x1b[H");
    assert!(out.is_empty());
  }

  #[test]
  fn erase_display_stripped() {
    let out = filter(b"\x1b[2J");
    assert!(out.is_empty());
  }

  #[test]
  fn tild_terminated_sequence_stripped() {
    let out = filter(b"\x1b[200~");
    assert!(out.is_empty());
  }

  #[test]
  fn mixed_text_and_sgr() {
    let out = filter(b"pass ");
    assert_eq!(out, b"pass ");
    let out = filter(b"\x1b[32m");
    assert_eq!(out, b"\x1b[32m");
    let out = filter(b"1");
    assert_eq!(out, b"1");
    let out = filter(b"\x1b[0m");
    assert_eq!(out, b"\x1b[0m");
    let out = filter(b" fail 2");
    assert_eq!(out, b" fail 2");
  }

  #[test]
  fn multiple_sgr_in_sequence() {
    let out = filter(b"\x1b[1m\x1b[31m\x1b[4m");
    assert_eq!(out, b"\x1b[1m\x1b[31m\x1b[4m");
  }

  #[test]
  fn text_sgr_text_dsr_text_sgr_text() {
    let out = filter(
      b"before\x1b[31mcolored\x1b[0m\x1b[21;1Rafter\x1b[32mgreen",
    );
    assert_eq!(
      out,
      b"before\x1b[31mcolored\x1b[0mafter\x1b[32mgreen"
    );
  }

  #[test]
  fn non_csi_escape_passed_through() {
    // ESC7 (save cursor), ESC8 (restore cursor)
    let out = filter(b"\x1b7\x1b8");
    assert_eq!(out, b"\x1b7\x1b8");
  }

  #[test]
  fn bare_esc_at_eof_flushed() {
    let out = filter(b"hello\x1b");
    assert_eq!(out, b"hello\x1b");
  }

  #[test]
  fn partial_csi_at_eof_flushed() {
    let out = filter(b"hello\x1b[");
    assert_eq!(out, b"hello\x1b[");
  }

  #[test]
  fn partial_csi_params_at_eof_flushed() {
    let out = filter(b"\x1b[31");
    assert_eq!(out, b"\x1b[31");
  }

  #[test]
  fn realistic_cargo_test_output() {
    let input = b"\x1b[0m\x1b[1m\x1b[32mok\x1b[0m\x1b[0m \x1b[1mtest_name\x1b[0m\n\x1b[1m\x1b[31mFAILED\x1b[0m\x1b[0m other_test\n\x1b[21;1R";
    let out = filter(input);
    assert_eq!(
      out,
      b"\x1b[0m\x1b[1m\x1b[32mok\x1b[0m\x1b[0m \x1b[1mtest_name\x1b[0m\n\x1b[1m\x1b[31mFAILED\x1b[0m\x1b[0m other_test\n"
    );
  }
}
