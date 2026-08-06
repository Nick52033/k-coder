# -*- coding: utf-8 -*-
"""从 Excel(.xlsx) 提取 Sheet 内容到文本文件。依赖: pip install pandas openpyxl"""
import os
import sys

try:
    import pandas as pd
except ImportError:
    print("请先安装: pip install pandas openpyxl")
    raise

def extract_xlsx(xlsx_path: str, out_path: str = None, sheet_name: int = 0) -> str:
    xlsx_path = os.path.abspath(xlsx_path)
    if not os.path.isfile(xlsx_path):
        raise FileNotFoundError("文件不存在: " + xlsx_path)
    if out_path is None:
        base, _ = os.path.splitext(xlsx_path)
        out_path = base + "_原文.txt"
    xl = pd.ExcelFile(xlsx_path)
    lines = ["Sheet names: " + str(xl.sheet_names), ""]
    df = pd.read_excel(xlsx_path, sheet_name=sheet_name, header=None)
    pd.set_option("display.max_columns", None)
    pd.set_option("display.width", 300)
    pd.set_option("display.max_rows", None)
    lines.append(df.to_string())
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    return out_path

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("用法: python extract_xlsx.py <需求文档.xlsx> [输出路径.txt] [sheet索引，默认0]")
        sys.exit(1)
    path = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else None
    sheet = int(sys.argv[3]) if len(sys.argv) > 3 else 0
    result = extract_xlsx(path, out, sheet_name=sheet)
    print("已写出:", result)
