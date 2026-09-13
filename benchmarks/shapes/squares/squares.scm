; The same chain, but the per-element term is (* i i). This exercises the
; element-substitution path: the balanced form has to carry `G` into the leaf.
(define (run i n acc)
  (if (> i n)
      acc
      (run (+ i 1) n (+ acc (* i i)))))
(run 1 200000 0)
