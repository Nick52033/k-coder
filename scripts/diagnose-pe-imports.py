"""Reports which DLL import makes a freshly built test binary unloadable.

The lib test binary here is ~73 MB and links the whole Tauri tree, so when the Windows loader
returns STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139) there is no message saying *which* import is
missing. This walks the PE import directory, loads every named DLL and resolves every imported
symbol through ctypes, then prints the ones that do not exist.
"""

import ctypes
import os
import struct
import sys


def sections(f):
    f.seek(0x3C)
    pe = struct.unpack("<I", f.read(4))[0]
    f.seek(pe)
    if f.read(4) != b"PE\0\0":
        raise SystemExit("not a PE file")
    machine, count, _, _, _, opt_size, _ = struct.unpack("<HHIIIHH", f.read(20))
    f.seek(pe + 24)
    optional = f.read(opt_size)
    magic = struct.unpack("<H", optional[:2])[0]
    data_dir = 112 if magic == 0x20B else 96
    import_rva = struct.unpack("<I", optional[data_dir + 8 : data_dir + 12])[0]
    f.seek(pe + 24 + opt_size)
    table = []
    for _ in range(count):
        raw = f.read(40)
        name = raw[:8].rstrip(b"\x00").decode(errors="replace")
        vsize, vaddr, rawsize, rawptr = struct.unpack("<IIII", raw[8:24])
        table.append((name, vaddr, vsize, rawptr, rawsize))
    return pe, import_rva, table


def rva_to_offset(table, rva):
    for _, vaddr, vsize, rawptr, rawsize in table:
        if vaddr <= rva < vaddr + max(vsize, rawsize):
            return rawptr + (rva - vaddr)
    return None


def read_c_string(f, offset):
    f.seek(offset)
    out = b""
    while True:
        ch = f.read(1)
        if ch in (b"", b"\x00"):
            return out.decode(errors="replace")
        out += ch


def imports(path):
    with open(path, "rb") as f:
        pe, import_rva, table = sections(f)
        offset = rva_to_offset(table, import_rva)
        found = []
        while offset:
            f.seek(offset)
            descriptor = f.read(20)
            if len(descriptor) < 20 or descriptor == b"\x00" * 20:
                break
            lookup_rva, _, _, name_rva, _ = struct.unpack("<IIIII", descriptor)
            if name_rva == 0:
                break
            dll = read_c_string(f, rva_to_offset(table, name_rva))
            names = []
            thunk = rva_to_offset(table, lookup_rva) if lookup_rva else None
            if thunk:
                cursor = thunk
                while True:
                    f.seek(cursor)
                    entry = struct.unpack("<Q", f.read(8))[0]
                    if entry == 0:
                        break
                    if entry & (1 << 63):
                        names.append(("ordinal", entry & 0xFFFF))
                    else:
                        names.append(("name", read_c_string(f, rva_to_offset(table, entry) + 2)))
                    cursor += 8
            found.append((dll, names))
            offset += 20
        return found


def main():
    path = sys.argv[1]
    print("file:", os.path.basename(path), os.path.getsize(path), "bytes")
    problems = []
    for dll, names in imports(path):
        try:
            handle = ctypes.WinDLL(dll)
        except OSError as error:
            problems.append(f"MISSING DLL {dll}: {error}")
            continue
        for kind, value in names:
            if kind == "ordinal":
                continue
            try:
                ctypes.cast(getattr(handle, value), ctypes.c_void_p)
            except AttributeError:
                problems.append(f"MISSING EXPORT {dll}!{value}")
    if problems:
        print("problems:")
        for problem in problems:
            print("  ", problem)
    else:
        print("every imported DLL and symbol resolves")


main()
