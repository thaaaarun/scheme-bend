; Sparse graph multiplication using adjacency-list rows.
;
; A row contains only nonzero (column, weight) edges. Matrix multiplication is
; therefore a two-hop graph walk: for every i->k edge in A, walk the k->j
; edges in B and accumulate A[i,k] * B[k,j] into output column j.
;
; The output row is kept sorted by column. `insert-add` combines paths that
; reach the same destination and removes entries that cancel back to zero.
; That aggregation is necessary before squaring: (x+y)^2 is not x^2+y^2.

(define (mod- a m) (- a (* (/ a m) m)))

; An edge is the two-element list (column weight).
(define (edge j w) (cons j (cons w ())))
(define (edge-col e) (car e))
(define (edge-weight e) (car (cdr e)))

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

; A rows are sorted by column, so join them with B's row list using a cursor.
; This advances through B at most once per output row instead of rescanning it
; from the beginning for every nonzero A edge.
(define (skip-rows count rows)
  (if (= count 0)
      rows
      (skip-rows (- count 1) (cdr rows))))

; Insert one contribution into a sorted sparse output row. The input
; representation already guarantees that surviving edge weights are nonzero,
; so the sparse hot path uses ordinary multiplication without a redundant zero
; test. `mul0` remains available for dense kernels with dynamic zero operands.
(define (insert-add j x acc)
  (if (= x 0)
      acc
      (if (null? acc)
          (cons (edge j x) ())
          (let ((e (car acc)))
            (let ((k (edge-col e)))
              (let ((v (edge-weight e)))
                (if (= j k)
                    (let ((s (+ v x)))
                      (if (= s 0)
                          (cdr acc)
                          (cons (edge k s) (cdr acc))))
                    (if (< j k)
                        (cons (edge j x) acc)
                        (cons e (insert-add j x (cdr acc)))))))))))

(define (scatter b-row scale acc)
  (if (null? b-row)
      acc
      (let ((e (car b-row)))
        (scatter (cdr b-row)
                 scale
                 (insert-add (edge-col e)
                             (* scale (edge-weight e))
                             acc)))))

(define (expand-row a-row b-rows previous-k acc)
  (if (null? a-row)
      acc
      (let ((e (car a-row)))
        (let ((k (edge-col e)))
          (let ((aik (edge-weight e)))
            (let ((b-at-k (skip-rows (- k previous-k) b-rows)))
              (expand-row (cdr a-row)
                          b-at-k
                          k
                          (scatter (car b-at-k) aik acc))))))))

(define (sq x) (mod- (* x x) 100000))

(define (square-sum row acc)
  (if (null? row)
      acc
      (let ((e (car row)))
        (let ((v (edge-weight e)))
          (square-sum (cdr row)
                      (mod- (+ acc (sq v)) 3000000))))))

; Each row is independent, so this outer recursion is the parallel frontier.
(define (rows-checksum a b-rows acc)
  (if (null? a)
      acc
      (rows-checksum (cdr a)
                     b-rows
                     (mod- (+ acc (square-sum (expand-row (car a) b-rows 0 ()) 0))
                           3000000))))

(define (run n)
  (let ((a (make-matrix (- n 1) n ())))
    (let ((b (make-matrix (- n 1) n ())))
      (rows-checksum a b 0))))

(define (once) (run 128))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (once)) 4000000))))

(repeat 4 0)
