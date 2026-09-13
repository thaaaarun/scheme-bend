; Matrix multiply, reduced to a checksum: sum over all output cells of C^2
; modulo a 24-bit-safe modulus, for n x n matrices of small ints.
;
; The checksum is deliberately NON-LINEAR. A plain sum of the entries is a
; linear functional -- sum(A*B) = sum_k (sum_i A[i][k]) * (sum_j B[k][j]) --
; so it can be computed in O(n^2) without multiplying any matrices, which
; would make this benchmark meaningless. Summing squares removes that shortcut.
;
; Parallel shape: each output row is computed independently, so the row
; recursion offers n independent tasks to HVM's redex pool. A balanced tree
; over the columns as well was measured and is worse on both axes (26.4M vs
; 10.5M interactions and less parallel speedup), because the split passes the
; row and the transposed matrix into both halves and duplicates them at every
; node. Row-level recursion already supplies all the parallelism available.
;
; Bounds: entries are 0..7, so a dot product is at most 49*32 = 1568 and its
; square at most 2,458,624, which fits signed 24-bit. The accumulator is
; reduced modulo 3,000,000 after every addition so partial sums cannot wrap.
(define (mod- a m) (- a (* (/ a m) m)))

(define (make-row i j acc)
  (if (< j 0)
      acc
      (make-row i (- j 1) (cons (mod- (+ (* i 7) (* j 5)) 8) acc))))

(define (make-matrix i n acc)
  (if (< i 0)
      acc
      (make-matrix (- i 1) n (cons (make-row i (- n 1) ()) acc))))

; Transpose by peeling the leftmost column off in one O(n) pass per column.
(define (heads m)
  (if (null? m) () (cons (car (car m)) (heads (cdr m)))))

(define (tails m)
  (if (null? m) () (cons (cdr (car m)) (tails (cdr m)))))

(define (transpose m)
  (if (null? m)
      ()
      (if (null? (car m))
          ()
          (cons (heads m) (transpose (tails m))))))

(define (nth-list k xs)
  (if (= k 0) (car xs) (nth-list (- k 1) (cdr xs))))

(define (length-of xs)
  (if (null? xs) 0 (+ 1 (length-of (cdr xs)))))

(define (dot xs ys acc)
  (if (null? xs) acc (dot (cdr xs) (cdr ys) (+ acc (* (car xs) (car ys))))))

; Walk the shrinking column list rather than indexing it. Indexing with
; nth-list re-reads the column each time *and* leaves `a` and the transposed
; matrix live across both the dot product and the recursive call, so eager HVM
; duplicates them once per cell -- measured at 6.8x the interactions.
(define (row-checksum a cols acc)
  (if (null? cols)
      acc
      (let ((c (dot a (car cols) 0)))
        (row-checksum a (cdr cols) (mod- (+ acc (mod- (* c c) 3000000)) 3000000)))))

(define (rows-checksum a bt acc)
  (if (null? a)
      acc
      (rows-checksum (cdr a)
                     bt
                     (mod- (+ acc (mod- (row-checksum (car a) bt 0) 3000000))
                           3000000))))

(define (run n)
  (rows-checksum (make-matrix (- n 1) n ()) (transpose (make-matrix (- n 1) n ())) 0))

; One run is far below SBCL's ~10ms process startup, so the kernel is repeated
; inside one process with a bounded 24-bit checksum. Both implementations run
; this identical wrapper.
(define (once) (run 32))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 50 0)
