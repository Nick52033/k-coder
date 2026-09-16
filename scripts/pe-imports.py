"""列出 PE 文件的静态导入 DLL 与函数名。

只依赖标准库：读 DOS 头 -> PE 头 -> 可选头 -> 导入目录 -> 遍历导入描述符与
OriginalFirstThunk 的 Hint/Name 表。

用于诊断 Windows 上二进制加载失败（0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND）：
若某个系统 DLL 的导入函数只存在于该 DLL 的新版本（典型是 comctl32.dll 的
TaskDialogIndirect 等 Common Controls v6 专有导出），而可执行文件没有相应的
清单（manifest）声明，加载器会把它解析到 System32 下的旧版本并报入口点找不到。

用法：
    python scripts/pe-imports.py <exe> [dll-name-filter]
"""

import struct
import sys


def read_imports(path):
    """返回 [(dll_name, [function_or_ordinal, ...]), ...]。"""
    with open(path, "rb") as handle:
        data = handle.read()

    if data[:2] != b"MZ":
        raise ValueError("not a PE file (missing MZ)")

    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise ValueError("not a PE file (missing PE signature)")

    coff = pe_offset + 4
    section_count = struct.unpack_from("<H", data, coff + 2)[0]
    optional_size = struct.unpack_from("<H", data, coff + 16)[0]
    optional = coff + 20

    magic = struct.unpack_from("<H", data, optional)[0]
    is_pe32_plus = magic == 0x20B

    # 数据目录紧跟在可选头固定部分之后；导入目录是第 2 项（索引 1）。
    dirs = optional + (112 if is_pe32_plus else 96)
    import_rva, _import_size = struct.unpack_from("<II", data, dirs + 8)
    if import_rva == 0:
        return []

    sections = []
    section_table = optional + optional_size
    for index in range(section_count):
        base = section_table + index * 40
        virtual_size, virtual_address, raw_size, raw_pointer = struct.unpack_from(
            "<IIII", data, base + 8
        )
        sections.append((virtual_address, virtual_size, raw_pointer, raw_size))

    def rva_to_offset(rva):
        for virtual_address, virtual_size, raw_pointer, raw_size in sections:
            span = max(virtual_size, raw_size)
            if virtual_address <= rva < virtual_address + span:
                return raw_pointer + (rva - virtual_address)
        return None

    def read_c_string(offset):
        end = data.index(b"\0", offset)
        return data[offset:end].decode("ascii", "replace")

    imports = []
    cursor = import_rva
    while True:
        offset = rva_to_offset(cursor)
        if offset is None:
            break
        original_first_thunk, _time, _forwarder, name_rva, first_thunk = struct.unpack_from(
            "<IIIII", data, offset
        )
        if original_first_thunk == 0 and name_rva == 0 and first_thunk == 0:
            break

        name_offset = rva_to_offset(name_rva)
        dll = read_c_string(name_offset) if name_offset is not None else "<unknown>"

        functions = []
        thunk_rva = original_first_thunk or first_thunk
        while thunk_rva:
            thunk_offset = rva_to_offset(thunk_rva)
            if thunk_offset is None:
                break
            entry = struct.unpack_from("<Q" if is_pe32_plus else "<I", data, thunk_offset)[0]
            if entry == 0:
                break
            # 最高位为 1 表示按序号导入。
            ordinal_flag = 1 << (63 if is_pe32_plus else 31)
            if entry & ordinal_flag:
                functions.append(f"#{entry & 0xFFFF}")
            else:
                hint_offset = rva_to_offset(entry)
                if hint_offset is not None:
                    functions.append(read_c_string(hint_offset + 2))
            thunk_rva += 8 if is_pe32_plus else 4

        imports.append((dll, functions))
        cursor += 20

    return imports


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1

    target = sys.argv[1]
    dll_filter = sys.argv[2].lower() if len(sys.argv) > 2 else None

    try:
        imports = read_imports(target)
    except Exception as error:  # noqa: BLE001 - 诊断脚本，直接报告即可
        print(f"{target}: ERROR {error}")
        return 1

    total = sum(len(functions) for _dll, functions in imports)
    print(f"{target}: {len(imports)} DLLs, {total} imported symbols")
    for dll, functions in sorted(imports, key=lambda item: item[0].lower()):
        if dll_filter and dll_filter not in dll.lower():
            continue
        print(f"  {dll} ({len(functions)})")
        for name in sorted(functions):
            print(f"      {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
