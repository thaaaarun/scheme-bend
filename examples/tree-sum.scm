; Trees are immutable pairs: (cons value (cons left right)); empty children are (). expected: 10
(define (tree-sum tree)
  (if (null? tree)
      0
      (+ (car tree) (+ (tree-sum (car (cdr tree))) (tree-sum (cdr (cdr tree)))))))
(tree-sum (cons 1
               (cons (cons 2 (cons () ()))
                     (cons 3 (cons (cons 4 (cons () ())) ())))))
