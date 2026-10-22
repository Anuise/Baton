# 用法：python -I normalize_decisions.py <ai-decisions.md>
# 確保每個條目（### AI-NN／### D-NN）都有一行「- 審閱：…」，並在條目之間補空行。可重複執行。
import re, sys

path = sys.argv[1]
lines = open(path, encoding="utf-8").read().split("\n")
out, block = [], None
in_code = False


def flush(block):
    # 依既有的認同／指定／修正行決定審閱狀態
    if any(l.startswith("- 審閱：") for l in block):
        return block
    fields, rest = [], []
    for l in block[1:]:
        (fields if not rest and l.startswith("- ") else rest).append(l)
    status = "未審"
    for i, l in enumerate(fields):
        m = re.match(r"- 使用者(認同|指定)：(\S+)", l)
        if m:
            status = f"認同 {m.group(2)}" + ("（使用者指定）" if m.group(1) == "指定" else "")
            fields.pop(i)
            break
    if status == "未審" and any("使用者修正：" in l for l in block):
        status = "已修正"
    return [block[0]] + fields + [f"- 審閱：{status}"] + rest


for l in lines:
    if l.startswith("```"):
        in_code = not in_code
    if not in_code and re.match(r"### (AI|D)-\d+", l):
        if block:
            out += flush(block)
        if out and out[-1].strip():
            out.append("")
        block = [l]
    elif block is not None and not in_code and l.startswith("## "):
        out += flush(block)
        block = None
        out.append(l)
    elif block is not None:
        block.append(l)
    else:
        out.append(l)
if block:
    out += flush(block)
open(path, "w", encoding="utf-8").write("\n".join(out))
