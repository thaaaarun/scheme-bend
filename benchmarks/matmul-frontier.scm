; The same runtime-generated sparse matrix workload as matmul-sparse.scm,
; routed through the compiler's dynamic Map-backed parallel chunk kernel.
;
; A row of A is a frontier of (k, A[i,k]) events. The adjacency Map stores
; B's sparse row for each k. The scatter kernel expands each A event directly
; into the output accumulator Map, with no materialized terminal frontier.

(define (mod- a m) (- a (* (/ a m) m)))

(define (edge j w) (cons j (cons w ())))

(define (present i j)
  (= (mod- (+ (* i 5) (* j 3)) 16) 0))

(define (weight i j)
  (+ 1 (mod- (+ (* i 3) (* j 5)) 7)))

(define (make-row i j acc)
  (if (< j 0)
      acc
      (if (present i j)
          (make-row i (- j 1) (cons (edge j (weight i j)) acc))
          (make-row i (- j 1) acc))))

(define (make-matrix i n acc)
  (if (< i 0)
      acc
      (make-matrix (- i 1) n (cons (make-row i (- n 1) ()) acc))))

(define (rows-to-map rows index acc)
  (if (null? rows)
      acc
      (rows-to-map (cdr rows)
                   (+ index 1)
                   (map-set acc index (car rows)))))

(define (run n)
  (let ((a (make-matrix (- n 1) n ())))
    (let ((b (make-matrix (- n 1) n ())))
      (let ((b-map (rows-to-map b 0 (map-empty))))
        (let ((tree (sparse-chunk-tree a n 32)))
          (sparse-parallel-chunks tree b-map n))))))

(define (once) (run 128))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (once)) 4000000))))

(repeat 4 0)
