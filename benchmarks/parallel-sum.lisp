;; Algorithmically equivalent native SBCL baseline.
(declaim (optimize (speed 3) (safety 0) (debug 0)))
(defun parallel-sum (lo hi)
  (declare (type fixnum lo hi))
  (if (= lo hi)
      lo
      (let ((mid (truncate (+ lo hi) 2)))
        (+ (parallel-sum lo mid)
           (parallel-sum (1+ mid) hi)))))
(compile 'parallel-sum)
(defun benchmark-result ()
  (logand (parallel-sum 1 1000000) #xFFFFFF))
(compile 'benchmark-result)
