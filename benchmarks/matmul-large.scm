; Scaled-up dense GEMM, reduced to a checksum: sum over every output cell of
; C^2 (modulo 100000), itself accumulated modulo 3,000,000.
;
; n = 256. That is 512x the work of benchmarks/matmul.scm's 32x32, and it
; exposes 256 independent output rows -- 16x oversubscription for a 15-core
; host -- each costing 65,536 multiply-adds, so the per-leaf work is heavy
; enough that the reduction overhead is negligible.
;
; Entries are quantized to 0..3, so a dot product over 256 terms is at most
; 256 * 9 = 2304 and its square at most 5,308,416, inside signed 24-bit. Each
; squared cell is reduced modulo 100000 and the row accumulator modulo
; 3,000,000 after every group of four, so no partial sum can wrap:
; 3,000,000 + 4 * 100,000 = 3,400,000 < 8,388,607. Every intermediate stays in
; range, which is also what lets the two backends agree exactly rather than
; only modulo 2^24.
;
; The checksum is deliberately NON-LINEAR. A plain sum of the entries is a
; linear functional -- sum(A*B) = sum_k (sum_i A[i][k]) * (sum_j B[k][j]) --
; so it could be computed in O(n^2) without multiplying any matrices, which
; would make the benchmark meaningless. Summing squares removes that shortcut.
;
; Parallel shape: each output row is independent, so the row recursion offers
; 256 independent tasks to HVM's redex pool. Four columns are consumed per
; traversal of the row, so the row is read once per four cells instead of once
; per cell -- the same list-sharing argument benchmarks/matmul.scm documents.
(define (mod- a m) (- a (* (/ a m) m)))

(define (make-row i j acc)
  (if (< j 0)
      acc
      (make-row i (- j 1) (cons (mod- (+ (* i 3) (* j 5)) 4) acc))))

(define (make-matrix i n acc)
  (if (< i 0)
      acc
      (make-matrix (- i 1) n (cons (make-row i (- n 1) ()) acc))))

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

(define (sq c) (mod- (* c c) 100000))

; Four dot products per traversal, returned as the sum of their squares. The
; accumulator here is bounded by 4 * 100000, so this cannot wrap either.
(define (quad-checksum xs y1 y2 y3 y4 a1 a2 a3 a4)
  (if (null? xs)
      (+ (+ (sq a1) (sq a2)) (+ (sq a3) (sq a4)))
      (quad-checksum (cdr xs) (cdr y1) (cdr y2) (cdr y3) (cdr y4)
                     (+ a1 (* (car xs) (car y1)))
                     (+ a2 (* (car xs) (car y2)))
                     (+ a3 (* (car xs) (car y3)))
                     (+ a4 (* (car xs) (car y4))))))

(define (add acc term) (mod- (+ acc term) 3000000))

(define (row-checksum a cols acc)
  (if (null? cols)
      acc
      (row-checksum a
                    (cdr (cdr (cdr (cdr cols))))
                    (add acc (quad-checksum a
                                            (car cols)
                                            (car (cdr cols))
                                            (car (cdr (cdr cols)))
                                            (car (cdr (cdr (cdr cols))))
                                            0 0 0 0)))))

(define (rows-checksum a bt acc)
  (if (null? a)
      acc
      (rows-checksum (cdr a)
                     bt
                     (add acc (row-checksum (car a) bt 0)))))

(define (run n)
  (rows-checksum (make-matrix (- n 1) n ())
                 (transpose (make-matrix (- n 1) n ()))
                 0))

(run 256)
