; expected: 2
(define xs (cons 1 (cons 2 (cons 3 ()))))
(car (cdr xs))
