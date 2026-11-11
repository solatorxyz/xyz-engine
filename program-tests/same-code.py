#!/usr/bin/env python3
"""Do two builds of the program contain the same machine code?

A refactor that only moves source text (formatting, splitting a file into modules) must not change what runs on chain.
The build is deterministic, but the two binaries still differ, for reasons that have nothing to do with behaviour:

  * every overflow check carries a panic location (file, line, column) in .rodata, so moving a line changes .rodata,
    and when .rodata changes size, every address the code loads from it shifts;
  * the compiler lays functions out in definition order, so moving a function to another file moves its code, and
    every call to it gets a different relative offset;
  * a function's symbol carries its module path.

This script removes exactly those three differences and nothing else. It takes the two unstripped builds
(target/sbpfv3-solana-solana/release/perp_program.so), cuts .text into functions by the symbol table, and rewrites
each function into a position-independent form: a call becomes "call <name of the callee>", a 64-bit load of a code
address becomes "<name of the function>", a 64-bit load of a data address becomes "<data>". Module names inside the
crate are dropped from the names. Then the two sets of functions must be equal: same names, same instructions, bit
for bit.

usage: same-code.py old.so new.so [old-modules [new-modules]]      exit 0 = same code
       modules: comma-separated names of the crate's own modules in that build, dropped from symbol paths.
       Defaults: none for the old build (the single-file program), every .rs file next to lib.rs for the new one.
"""
import os, re, struct, subprocess, sys, glob, collections

OBJDUMP = os.environ.get("OBJDUMP") or next(iter(sorted(glob.glob(os.path.expanduser("~/.cache/solana/*/platform-tools/llvm/bin/llvm-objdump")), reverse=True)), "llvm-objdump")
CRATE = "perp_program"

def functions(path, modules):
    """[(normalised name, start, size)] and the .text bytes with its load address."""
    raw = open(path, "rb").read()
    shoff, = struct.unpack_from("<Q", raw, 0x28); entsize, n, strndx = struct.unpack_from("<HHH", raw, 0x3A)
    sections = [struct.unpack_from("<IIQQQQIIQQ", raw, shoff + i * entsize) for i in range(n)]
    name_of = lambda s: raw[sections[strndx][4] + s[0]:].split(b"\0", 1)[0]
    text = next(s for s in sections if name_of(s) == b".text"); addr, off, size = text[3], text[4], text[5]
    drop = re.compile(r"\b%s::(?:%s)::" % (CRATE, "|".join(map(re.escape, modules)))) if modules else None
    out = []
    for line in subprocess.run([OBJDUMP, "-t", "-C", path], capture_output=True, text=True, check=True).stdout.splitlines():
        m = re.match(r"([0-9a-f]{16}) .{7} \.text\t([0-9a-f]{16}) (.+)$", line)
        if not m or " F " not in line[:26]: continue
        name = m.group(3); name = drop.sub(CRATE + "::", name) if drop else name
        out.append((name, int(m.group(1), 16), int(m.group(2), 16)))
    # two source functions with identical code share one address: one entry, carrying both names
    merged = {}
    for name, a, sz in out: merged.setdefault((a, sz), []).append(name)
    out = sorted(((" = ".join(sorted(names)), a, sz) for (a, sz), names in merged.items()), key=lambda f: f[1])
    covered = sum(f[2] for f in out)
    if covered != size: raise SystemExit(f"{path}: symbols cover {covered} of {size} bytes of .text (stripped build?)")
    return out, raw[off:off + size], addr

def normalise(path, modules):
    funcs, text, base = functions(path, modules)
    start = {a: nm for nm, a, _ in funcs}
    res = collections.Counter(); slots = 0
    for name, a, size in funcs:
        code = text[a - base:a - base + size]; form = []; i = 0
        while i < size:
            ins = code[i:i + 8]; op, regs = ins[0], ins[1]
            if op == 0x18:                                  # lddw: a 64-bit immediate over two slots
                imm = struct.unpack_from("<I", ins, 4)[0] | struct.unpack_from("<I", code, i + 12)[0] << 32
                target = start.get(imm) if base <= imm < base + len(text) else "<data>"
                if target is None: raise SystemExit(f"{path}: {name}+{i:#x} loads a code address that is not a function")
                form.append((ins[:4], "addr", target)); i += 16; slots += 2; continue
            if op == 0x85 and regs >> 4 == 1:               # call to a function of the program, relative to the next slot
                target = start.get(a + i + 8 + struct.unpack_from("<i", ins, 4)[0] * 8)
                if target is None: raise SystemExit(f"{path}: {name}+{i:#x} calls into the middle of a function")
                form.append((ins[:4], "call", target))
            else: form.append(ins)
            i += 8; slots += 1
        res[(name, tuple(form))] += 1
    return res, len(funcs), slots

def main(old, new, old_modules="", new_modules=None):
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../program/src/*.rs")
    if new_modules is None: new_modules = ",".join(os.path.basename(p)[:-3] for p in glob.glob(here) if not p.endswith(("lib.rs", "state.rs", "proofs.rs")))
    split = lambda s: [m for m in s.split(",") if m]
    (a, na, sa), (b, nb, sb) = normalise(old, split(old_modules)), normalise(new, split(new_modules))
    if a == b:
        print(f"same code: {na} functions, {sa} instruction slots, identical after removing layout (addresses, call offsets, module names)")
        return
    names_a, names_b = {k[0] for k in a}, {k[0] for k in b}
    for nm in sorted(names_a - names_b)[:10]: print("only in old:", nm[:150])
    for nm in sorted(names_b - names_a)[:10]: print("only in new:", nm[:150])
    for nm in sorted({k[0] for k in (a - b)} & {k[0] for k in (b - a)})[:10]: print("body differs:", nm[:150])
    raise SystemExit(f"DIFFERENT: {na} vs {nb} functions, {sa} vs {sb} slots")

if __name__ == "__main__":
    main(*sys.argv[1:])
