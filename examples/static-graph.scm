; A static weighted DAG. Nodes 8 and 9 form an unreachable cycle and are
; removed before Bend is emitted.
(define-graph layer
  (nodes 10)
  (inputs (0 1 2))
  (outputs (6 7))
  (edges
    (0 3 2)
    (1 3 1)
    (1 4 3)
    (2 4 -1)
    (3 5 2)
    (4 5 1)
    (5 6 1)
    (2 7 4)
    (8 9 1)
    (9 8 1)))

(define (sum xs acc)
  (if (null? xs)
      acc
      (sum (cdr xs) (+ acc (car xs)))))

(sum (layer 2 3 5) 0)
