import { execFileSync } from "child_process";
import { pathToFileURL } from "url";

/** One attempt at handing the OS clipboard a file. */
export interface ClipboardCommand {
  cmd: string;
  args: string[];
  /** Fed to the tool on stdin. Empty when the path travels in argv. */
  input: string;
}

/**
 * Commands that put `filePath` on the clipboard as a *file* (a uri-list, i.e. a
 * file drop Brickadia can paste) rather than as text, best-first: the caller
 * walks the list until one exits cleanly.
 *
 * On Linux the right tool depends on the session's display server. xclip talks
 * X11 only, so on Wayland it either fails outright (no XWayland) or copies into
 * XWayland's clipboard, where wayland-native apps never see it -- wl-copy is
 * the one that reaches them. DISPLAY is usually set under Wayland too (for
 * XWayland), so WAYLAND_DISPLAY is what actually distinguishes the two; each
 * tool stays as the other's fallback since either may be the one installed.
 */
export function clipboardCommands(
  filePath: string,
  platform: NodeJS.Platform = process.platform,
  env: NodeJS.ProcessEnv = process.env,
): ClipboardCommand[] {
  if (platform === "win32") {
    return [
      {
        cmd: "powershell",
        args: [
          "-NoProfile",
          "-Command",
          `Set-Clipboard -Path '${filePath.replace(/'/g, "''")}'`,
        ],
        input: "",
      },
    ];
  }
  if (platform === "darwin") {
    return [
      {
        cmd: "osascript",
        args: ["-e", `set the clipboard to POSIX file "${filePath}"`],
        input: "",
      },
    ];
  }

  // RFC 2483: a uri-list is CRLF-terminated absolute URIs. pathToFileURL
  // percent-encodes, so paths with spaces or '#' survive the round trip -- the
  // old hand-built `file://${outPath}` did not.
  const uri = pathToFileURL(filePath).href + "\r\n";
  const wayland: ClipboardCommand = {
    cmd: "wl-copy",
    args: ["--type", "text/uri-list"],
    input: uri,
  };
  const x11: ClipboardCommand = {
    cmd: "xclip",
    args: ["-selection", "clipboard", "-t", "text/uri-list", "-i"],
    input: uri,
  };
  return env.WAYLAND_DISPLAY ? [wayland, x11] : [x11, wayland];
}

/**
 * Copy `filePath` to the clipboard as a file drop. Returns false if no helper
 * on this machine could do it, so the caller can fall back to copying text.
 */
export function copyFileToClipboard(
  filePath: string,
  platform: NodeJS.Platform = process.platform,
  env: NodeJS.ProcessEnv = process.env,
): boolean {
  for (const { cmd, args, input } of clipboardCommands(filePath, platform, env)) {
    try {
      execFileSync(cmd, args, {
        input,
        // xclip and wl-copy hand the selection to a forked background process
        // that lives until another app claims the clipboard. That child
        // inherits our stdio, so a captured stdout/stderr pipe never reaches
        // EOF and execFileSync blocks long after the copy landed -- this is
        // what stranded the compile spinner on "Compiling: 4/4". Discarding the
        // child's output lets the call return at once; the exit status still
        // reports the failures we care about (tool missing, no display).
        stdio: ["pipe", "ignore", "ignore"],
        // Belt and braces: never let a wedged helper hang the extension host.
        timeout: 5000,
      });
      return true;
    } catch {
      // Try the next helper; the caller reports the all-failed case.
    }
  }
  return false;
}
