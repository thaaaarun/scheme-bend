;; Native SBCL baseline for benchmarks/fib.scm.
(declaim (optimize (speed 3) (safety 0) (debug 0)))
(defun fib (n)
  (declare (type fixnum n))
  (if (< n 2)
      n
      (+ (fib (- n 1)) (fib (- n 2)))))
(compile 'fib)
