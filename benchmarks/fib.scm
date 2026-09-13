; Deliberately naive recursion: enough parallel work for HVM, identical to fib.lisp.
(define (fib n)
  (if (< n 2)
      n
      (+ (fib (- n 1)) (fib (- n 2)))))
(fib 34)
