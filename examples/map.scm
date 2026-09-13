; expected: 2. This is the closure-free v0 form of map: specialize the mapper.
(define (map-inc xs)
  (if (null? xs)
      ()
      (cons (+ (car xs) 1) (map-inc (cdr xs)))))
(car (map-inc (cons 1 (cons 2 ()))))
