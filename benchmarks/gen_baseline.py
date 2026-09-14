#!/usr/bin/env python3
"""Generate the SBCL baseline for a benchmark from its Scheme source.

There is exactly one source of truth per benchmark: NAME.scm. The SBCL baseline
is derived mechanically, because hand-maintained twins drift -- every mismatch
found in the old layout (a silent 24-bit overflow, `floor` vs `truncate`, renamed
helpers) came from keeping two copies in sync by eye.

Usage: gen_baseline.py NAME.scm   -> prints the Lisp to stdout
"""
import sys

# Common Lisp symbols that must not be rebound. `t` and `nil` cannot be rebound
# at all; the rest would shadow a standard function.
CL_TAKEN = {'t', 'nil', 'count', 'gcd', 'mod', 'sum', 'abs', 'max', 'min',
            'list', 'length', 'reverse', 'append', 'last', 'first', 'expt',
            'mapcar', 'remove', 'position', 'sort', 'floor', 'truncate',
            'evenp', 'oddp', 'zerop', 'identity'}

ARITH = {'+', '-', '*', 'mul0', '/', '=', '<', '>', '<=', '>='}
LIST_TAKERS = {'car', 'cdr', 'null?'}

def parse(src):
    toks, i = [], 0
    while i < len(src):
        c = src[i]
        if c == ';':
            while i < len(src) and src[i] != '\n': i += 1
        elif c in '()':
            toks.append(c); i += 1
        elif c.isspace():
            i += 1
        else:
            j = i
            while j < len(src) and not src[j].isspace() and src[j] not in '();': j += 1
            toks.append(src[i:j]); i = j
    pos = 0
    def rd():
        nonlocal pos
        t = toks[pos]; pos += 1
        if t == '(':
            out = []
            while toks[pos] != ')': out.append(rd())
            pos += 1
            return out
        return t
    forms = []
    while pos < len(toks): forms.append(rd())
    return forms

def safe(name):
    return name + '_' if name in CL_TAKEN else name

def locals_of(node, acc):
    """Collect every name bound by a let/lambda inside a function body."""
    if not isinstance(node, list) or not node:
        return
    if node[0] == 'let' and len(node) > 1:
        for b in node[1]:
            if isinstance(b, list) and b: acc.add(b[0])
    for c in node:
        if isinstance(c, list): locals_of(c, acc)

def scan(node, listy, numeric):
    """Classify variable uses: which names are known to be lists vs numbers."""
    if not isinstance(node, list) or not node:
        return
    head = node[0] if isinstance(node[0], str) else None
    if head in LIST_TAKERS:
        for v in node[1:]:
            if isinstance(v, str): listy.add(v)
    if head == 'cons' and len(node) > 2 and isinstance(node[2], str):
        listy.add(node[2])              # the tail of a cons is a list
    if head in ARITH:
        for v in node[1:]:
            if isinstance(v, str): numeric.add(v)
    for c in node:
        if isinstance(c, list): scan(c, listy, numeric)

def render(node, ren):
    if not isinstance(node, list):
        if node == '()': return 'nil'
        if node == '#t': return 't'
        if node == '#f': return 'nil'
        if node and node[0] == '+' and node[1:].isdigit(): return node[1:]
        return ren.get(node, node)
    if not node: return 'nil'
    head = node[0]
    head = ren.get(head, head) if isinstance(head, str) else head
    if head == '/':
        return f"(truncate {render(node[1], ren)} {render(node[2], ren)})"
    if head == 'mul0':
        # Common Lisp's arithmetic is strict, but benchmark operands are total
        # and this preserves the numeric result for the reference run.
        return f"(* {render(node[1], ren)} {render(node[2], ren)})"
    if head == 'null?':
        return f"(null {render(node[1], ren)})"
    if head == 'map-empty':
        return "(make-hash-table :test #'eql)"
    if head == 'map-get':
        return f"(gethash {render(node[2], ren)} {render(node[1], ren)} {render(node[3], ren)})"
    if head == 'map-set':
        return f"(sb-map-set {render(node[1], ren)} {render(node[2], ren)} {render(node[3], ren)})"
    if head == 'sparse-frontier':
        return f"(sb-sparse-frontier {render(node[1], ren)} {render(node[2], ren)} {render(node[3], ren)})"
    if head == 'sparse-scatter':
        return f"(sb-sparse-scatter {render(node[1], ren)} {render(node[2], ren)} {render(node[3], ren)})"
    if head == 'sparse-square-sum':
        return f"(sb-sparse-square-sum {render(node[1], ren)} {render(node[2], ren)})"
    if head == 'sparse-parallel-rows':
        return f"(sb-sparse-parallel-rows {render(node[1], ren)} {render(node[2], ren)} {render(node[3], ren)} {render(node[4], ren)} {render(node[5], ren)})"
    if head == 'sparse-row-tree':
        return f"(sb-sparse-row-tree {render(node[1], ren)} {render(node[2], ren)})"
    if head == 'sparse-parallel-tree':
        return f"(sb-sparse-parallel-tree {render(node[1], ren)} {render(node[2], ren)})"
    if head == 'sparse-chunk-tree':
        return f"(sb-sparse-chunk-tree {render(node[1], ren)} {render(node[2], ren)} {render(node[3], ren)})"
    if head == 'sparse-parallel-chunks':
        return f"(sb-sparse-parallel-chunks {render(node[1], ren)} {render(node[2], ren)} {render(node[3], ren)})"
    if head == 'let':
        bindings = ' '.join(f"({render(b[0], ren)} {render(b[1], ren)})" for b in node[1])
        return f"(let ({bindings}) {render(node[2], ren)})"
    inner = ' '.join(render(c, ren) for c in node[1:])
    return f"({head} {inner})" if inner else f"({head})"

def main():
    src_path = sys.argv[1]
    forms = parse(open(src_path).read())
    defs = [f for f in forms if isinstance(f, list) and f[0] == 'define' and isinstance(f[1], list)]
    globals_ = [f for f in forms if isinstance(f, list) and f[0] == 'define' and not isinstance(f[1], list)]
    entry = [f for f in forms if not (isinstance(f, list) and f[0] == 'define')]
    assert len(entry) == 1, f"expected one top-level entry expression, found {len(entry)}"

    # Names visible everywhere: defined functions and top-level values.
    global_ren = {}
    for f in defs:  global_ren[f[1][0]] = safe(f[1][0])
    for f in globals_: global_ren[f[1]] = safe(f[1])

    out = [f";; GENERATED from {src_path} by gen_baseline.py -- do not edit.",
           ";; The Scheme file is the single source of truth for this benchmark.",
           "(declaim (optimize (speed 3) (safety 0) (debug 0)))", ""]
    out += [
        "(defun sb-map-set (map key value)",
        "  (setf (gethash key map) value)",
        "  map)",
        "",
        "(defun sb-sparse-scatter (adjacency frontier state)",
        "  (dolist (event frontier state)",
        "    (let ((node (first event)) (value (second event)))",
        "      (unless (zerop value)",
        "        (dolist (edge (gethash node adjacency))",
        "          (let ((contribution (* value (second edge))))",
        "            (unless (zerop contribution)",
        "              (incf (gethash (first edge) state 0) contribution))))))))",
        "",
        "(defun sb-sparse-frontier (adjacency frontier state)",
        "  (loop while frontier do",
        "    (let* ((event (pop frontier))",
        "           (node (first event))",
        "           (value (second event)))",
        "      (unless (zerop value)",
        "        (dolist (edge (gethash node adjacency))",
        "          (let ((contribution (* value (second edge))))",
        "            (unless (zerop contribution)",
        "              (incf (gethash (first edge) state 0) contribution)",
        "              (push (list (first edge) contribution) frontier)))))))",
        "  state)",
        "",
        "(defun sb-sparse-square-sum (state n)",
        "  (let ((acc 0))",
        "    (dotimes (column n acc)",
        "      (let* ((value (gethash column state 0))",
        "             (square (mod (* value value) 100000)))",
        "        (setf acc (mod (+ acc square) 3000000))))))",
        "",
        "(defun sb-sparse-parallel-rows (rows adjacency size first last)",
        "  (if (> first last)",
        "      0",
        "      (if (= first last)",
        "          (let ((acc 0))",
        "            (let ((state (make-hash-table :test #'eql)))",
        "              (dolist (event (gethash first rows))",
        "                (let ((node (first event)) (value (second event)))",
        "                  (unless (zerop value)",
        "                    (dolist (edge (gethash node adjacency))",
        "                      (incf (gethash (first edge) state 0)",
        "                            (* value (second edge)))))))",
        "              (dotimes (column size acc)",
        "                (let ((value (gethash column state 0)))",
        "                  (setf acc (mod (+ acc (mod (* value value) 100000)) 3000000))))))",
        "          (let* ((middle (truncate (+ first last) 2))",
        "                 (left (sb-sparse-parallel-rows rows adjacency size first middle))",
        "                 (right (sb-sparse-parallel-rows rows adjacency size (+ middle 1) last)))",
        "            (mod (+ left right) 3000000)))))",
        "",
        "(defun sb-sparse-row-tree (rows count)",
        "  (declare (ignore count))",
        "  rows)",
        "",
        "(defun sb-sparse-parallel-tree (tree size)",
        "  (let ((acc 0))",
        "    (dolist (prepared-row tree acc)",
        "      (let ((state (make-hash-table :test #'eql)))",
        "        (dolist (item prepared-row)",
        "          (let ((value (first item)))",
        "            (unless (zerop value)",
        "              (dolist (edge (second item))",
        "                (incf (gethash (first edge) state 0)",
        "                      (* value (second edge)))))))",
        "        (dotimes (column size)",
        "          (let ((value (gethash column state 0)))",
        "            (setf acc (mod (+ acc (mod (* value value) 100000)) 3000000))))))))",
        "",
        "(defun sb-sparse-chunk-tree (rows count chunk-size)",
        "  (declare (ignore count chunk-size))",
        "  rows)",
        "",
        "(defun sb-sparse-parallel-chunks (tree adjacency size)",
        "  (let ((acc 0))",
        "    (dolist (row tree acc)",
        "      (let ((state (make-hash-table :test #'eql)))",
        "        (dolist (event row)",
        "          (dolist (edge (gethash (first event) adjacency))",
        "            (incf (gethash (first edge) state 0)",
        "                  (* (second event) (second edge)))))",
        "        (dotimes (column size)",
        "          (let ((value (gethash column state 0)))",
        "            (setf acc (mod (+ acc (mod (* value value) 100000)) 3000000))))))))",
        "",
    ]
    for f in globals_:
        out.append(f"(defparameter {global_ren[f[1]]} {render(f[2], global_ren)})")
        out.append("")
    for f in defs:
        name, params = global_ren[f[1][0]], f[1][1:]
        locals_ = set()
        locals_of(f[2], locals_)
        ren = dict(global_ren)
        for v in list(params) + sorted(locals_):
            ren[v] = safe(v)
        listy, numeric = set(), set()
        scan(f[2], listy, numeric)
        # Declare only parameters provably numeric: used in arithmetic and never
        # used as a list. Guessing here would be unsound.
        fix = [p for p in params if p in numeric and p not in listy]
        argl = ' '.join(ren[p] for p in params)
        out.append(f"(defun {name} ({argl})")
        if fix:
            out.append(f"  (declare (fixnum {' '.join(ren[p] for p in fix)}))")
        out.append(f"  {render(f[2], ren)})")
        out.append("")
    out.append(f"(defun benchmark-result () {render(entry[0], global_ren)})")
    out.append("(compile 'benchmark-result)")
    print('\n'.join(out))

if __name__ == '__main__':
    main()
