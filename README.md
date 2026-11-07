# SVWriteBuffer

A DLL for the WoW 1.12 client that makes saving addon data (SavedVariables) much faster.
`/reload` and logout get quicker. The more your addons save, the more you'll notice.

## What it does

The client saves addon data one tiny piece at a time. Every key, bracket and value is a
separate write to disk. A big auction addon alone takes over 450,000 of them.
SVWriteBuffer collects each file in memory and writes it in one go.

On a UI with an auction addon, big unit frames and a bag addon:

| | Without | With |
|---|---|---|
| Writes for the 6.4 MB auction addon file | 457,000 | 1 |
| Time before addons start loading on `/reload` | 2.7 s | 1.2 s |
| Whole `/reload` | 5.2 s | 3.75 s |

With only small addons, you may not notice a difference.

## Is it safe?

- Your saved files are byte for byte the same. Only the number of writes changes.
- Each file is on disk before the client moves on, as without the DLL. Nothing is
  written in the background, so a crash can't lose more than before.
- Only SavedVariables files are buffered. Other files are written as usual.
- It only changes the running game process, in memory. Nothing on disk is modified.
- If another DLL already hooks a file function it needs, it leaves that one alone. If it
  can't cover opening, writing and closing, it stays off.
- It doesn't read or change game memory, Lua or network traffic.
- The release DLL is built from this repo by GitHub. Each release lists its SHA-256 and
  how to verify where it was built. Antivirus may still flag it, since it's an unsigned
  DLL that changes how another program behaves.

Check your server's rules first. Some only allow approved DLLs.

## Install

You need a client with a DLL loader that reads `dlls.txt` in the game folder. Most
community 1.12 clients have one.

1. Download `SVWriteBuffer.dll` from the
   [latest release](https://github.com/daev-code/SVWriteBuffer/releases/latest).
2. Put it in the game folder, next to the game's executable.
3. Add a line `SVWriteBuffer.dll` to `dlls.txt`.
4. Restart the game. `/reload` isn't enough, because DLLs only load at startup.

Some launchers rewrite `dlls.txt` on updates. If the log stops showing up, check that
the line is still there.

## Check that it works

The DLL writes `SVWriteBuffer.log` in the game folder. It starts fresh every time the game
starts, and stops growing at 1 MB. After a `/reload` it looks like this:

```
SVWriteBuffer 0.1.0
active: patched kernel32 CloseHandle ReadFile SetFilePointer ... CreateFileW
  6438184 bytes   456930 writes -> 1      1.3 ms  aux-addon.lua (account)
   690980 bytes   146909 writes -> 1      0.2 ms  pfUI.lua (character)
```

The first line is the version. After the first two lines, each line is one saved file:
size, writes the client made, writes that hit the disk, time taken, and the file name.
If the second line says `not active`, the reason follows and the game runs as if the DLL
weren't there.

The log never contains folder paths, so no account, realm or character names. It's safe
to attach to an issue.

You can also see the version under the DLL's Properties > Details.

## Uninstall

Remove the line from `dlls.txt` and restart the game. Then delete the DLL and the log.

## Feedback

Found a problem? [Open an issue](https://github.com/daev-code/SVWriteBuffer/issues) and
attach your `SVWriteBuffer.log`. To build it yourself or see how it works, read
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
