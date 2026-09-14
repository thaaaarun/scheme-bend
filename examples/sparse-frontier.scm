; Runtime sparse propagation. The adjacency is a Map from node ids to edge
; lists, and the frontier contains (node, activation) events.
(define (edge destination weight)
  (cons destination (cons weight ())))

(define adjacency0 (map-empty))
(define adjacency1
  (map-set adjacency0 0
           (cons (edge 1 2)
                 (cons (edge 2 1) ()))))
(define adjacency2
  (map-set adjacency1 1
           (cons (edge 3 3) ())))
(define adjacency
  (map-set adjacency2 2
           (cons (edge 3 4) ())))

(define frontier
  (cons (cons 0 (cons 5 ())) ()))

(define result
  (sparse-frontier adjacency frontier (map-empty)))

(map-get result 3 0)
