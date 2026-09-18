"""Which import of a Windows test binary the loader cannot resolve.

A test executable that exits 0xc0000139 (STATUS_ENTRYPOINT_NOT_FOUND) before
running a line is a binary whose import table names a function that the DLL
it was resolved to does not export. Windows says nothing about which one.
This asks the same loader the same questions, one import at a time: every
DLL each executable imports is loaded with the executable's own directory on
the search path, and every imported name (or ordinal) is looked up in it.
Whatever does not resolve is printed. Diagnostic only: the exit code is 0.

Usage: python unresolved_imports.py <directory with the test executables>
"""

import ctypes
import ctypes.wintypes as wt
import glob
import os
import sys

import pefile

LOAD_LIBRARY_SEARCH_DEFAULT_DIRS = 0x00001000
LOAD_LIBRARY_SEARCH_USER_DIRS = 0x00000400

kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
kernel32.LoadLibraryExW.restype = wt.HMODULE
kernel32.LoadLibraryExW.argtypes = [wt.LPCWSTR, wt.HANDLE, wt.DWORD]
kernel32.GetProcAddress.restype = ctypes.c_void_p
kernel32.GetProcAddress.argtypes = [wt.HMODULE, ctypes.c_char_p]
kernel32.FreeLibrary.argtypes = [wt.HMODULE]
kernel32.SetErrorMode.restype = wt.UINT
kernel32.SetErrorMode.argtypes = [wt.UINT]


def imports_of(path):
    """Every (dll, [names]) the executable imports, delay-loaded ones included."""
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(
        directories=[
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"],
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"],
        ]
    )
    entries = list(getattr(pe, "DIRECTORY_ENTRY_IMPORT", []))
    entries += list(getattr(pe, "DIRECTORY_ENTRY_DELAY_IMPORT", []))
    return [(entry.dll.decode(errors="replace"), entry.imports) for entry in entries]


def unresolved(path):
    problems = []
    for dll, imports in imports_of(path):
        handle = kernel32.LoadLibraryExW(
            dll, None, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS
        )
        if not handle:
            problems.append(f"{dll}: cannot be loaded (error {ctypes.get_last_error()})")
            continue
        for imported in imports:
            if imported.name:
                wanted = imported.name.decode(errors="replace")
                found = kernel32.GetProcAddress(handle, imported.name)
            else:
                wanted = f"#{imported.ordinal}"
                found = kernel32.GetProcAddress(handle, ctypes.c_char_p(imported.ordinal))
            if not found:
                problems.append(f"{dll}: {wanted} is not exported")
        kernel32.FreeLibrary(handle)
    return problems


def import_names(path):
    """Every (dll, name) pair the executable imports, for comparing binaries."""
    names = set()
    for dll, imports in imports_of(path):
        for imported in imports:
            wanted = imported.name.decode(errors="replace") if imported.name else f"#{imported.ordinal}"
            names.add((dll.lower(), wanted))
    return names


def compare(exe, others):
    """What `exe` imports that none of `others` do: the code paths only it has."""
    mine = import_names(exe)
    theirs = set()
    for other in others:
        theirs |= import_names(other)
    extra = sorted(mine - theirs)
    print(f"{os.path.basename(exe)} imports {len(extra)} thing(s) that {', '.join(os.path.basename(o) for o in others)} do not:")
    for dll, name in extra:
        print(f"  {dll}: {name}")


def run_with_hard_errors_shown(exe, timeout=60):
    """Runs the executable with the loader's hard-error popup allowed.

    On a non-interactive runner the popup cannot be seen, but showing it logs
    an "Application Popup" event (id 26) in the System log carrying the
    loader's own sentence — "The procedure entry point X could not be located
    in the dynamic link library Y" — which the workflow reads afterwards. The
    popup may hang the process, so it is killed after `timeout` seconds.
    """
    import subprocess

    kernel32.SetErrorMode(0)
    print(f"running {os.path.basename(exe)} --list with hard errors shown")
    try:
        completed = subprocess.run([exe, "--list"], capture_output=True, timeout=timeout, text=True)
        code = completed.returncode & 0xFFFFFFFF
        print(f"  exit code 0x{code:08x}")
        if completed.stdout.strip():
            print("  stdout: " + completed.stdout.strip()[:500])
        if completed.stderr.strip():
            print("  stderr: " + completed.stderr.strip()[:500])
    except subprocess.TimeoutExpired:
        print(f"  did not exit within {timeout}s (a popup is probably up); killed")


def main():
    directory = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else r"target\debug\deps")
    # The executable's own directory is searched first, the way the loader
    # does it for the executable itself.
    os.add_dll_directory(directory)
    parent = os.path.dirname(directory)
    if os.path.isdir(parent):
        os.add_dll_directory(parent)

    dlls = sorted(glob.glob(os.path.join(directory, "*.dll")))
    print(f"DLLs in {directory}: {', '.join(os.path.basename(d) for d in dlls) or '(none)'}")

    exes = sorted(glob.glob(os.path.join(directory, "*.exe")))
    for exe in exes:
        try:
            problems = unresolved(exe)
        except Exception as error:  # noqa: BLE001 - a diagnostic reports, never fails
            print(f"{os.path.basename(exe)}: could not be read ({error})")
            continue
        if problems:
            print(f"{os.path.basename(exe)}: {len(problems)} unresolved import(s)")
            for problem in problems:
                print(f"  {problem}")
        else:
            print(f"{os.path.basename(exe)}: every import resolves")

    # The desktop IPC test binary against the desktop binaries that do start:
    # what it alone imports, and what the loader says when it is allowed to.
    suspects = [exe for exe in exes if os.path.basename(exe).startswith("commands-")]
    controls = [
        exe
        for exe in exes
        if os.path.basename(exe).startswith(("eavery_desktop-", "eavery_desktop_lib-"))
    ]
    for exe in suspects:
        try:
            if controls:
                compare(exe, controls)
            run_with_hard_errors_shown(exe)
        except Exception as error:  # noqa: BLE001
            print(f"{os.path.basename(exe)}: diagnostic failed ({error})")


if __name__ == "__main__":
    main()
