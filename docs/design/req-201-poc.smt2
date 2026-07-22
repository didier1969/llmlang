; PoC REQ-201 : encodage prédicat-récursif pour `forall x in xs: x >= 0`
(declare-datatypes ((Lst 1)) ((par (T) ((nil) (cons (head T) (tail (Lst T)))))))
; sum_Int (déjà dans le compilo)
(declare-fun sum_Int ((Lst Int)) Int)
(assert (= (sum_Int (as nil (Lst Int))) 0))
(assert (forall ((h Int) (t (Lst Int))) (! (= (sum_Int (cons h t)) (+ h (sum_Int t))) :pattern ((sum_Int (cons h t))))))
; allnn : le prédicat récursif généré pour `forall x in _: x >= 0` (0 free var)
(declare-fun allnn ((Lst Int)) Bool)
(assert (= (allnn (as nil (Lst Int))) true))
(assert (forall ((h Int) (t (Lst Int))) (! (= (allnn (cons h t)) (and (>= h 0) (allnn t))) :pattern ((allnn (cons h t))))))

; ---- VC de la branche cons de nonneg_sum : xs = (cons p_h p_t) ----
(declare-const p_h Int)
(declare-const p_t (Lst Int))
; requires (caller) : allnn(xs) = allnn(cons p_h p_t)
(assert (allnn (cons p_h p_t)))
; appel récursif nonneg_sum(p_t) : son requires allnn(p_t) DOIT être prouvé (obligation #A),
; puis son résultat r_rec est havocé avec ses ensures (r_rec == sum(p_t) et r_rec >= 0).
(declare-const r_rec Int)
; (obligation A : le call-site requires) — vérifions-la séparément plus bas.
; on ASSUME les ensures du callee (contract firewall) :
(assert (= r_rec (sum_Int p_t)))
(assert (>= r_rec 0))
; result = p_h + r_rec
(define-fun result () Int (+ p_h r_rec))

(push)
; OBLIGATION 1 (ensures result == sum(xs)) : nier -> doit être UNSAT
(assert (not (= result (sum_Int (cons p_h p_t)))))
(check-sat)
(pop)
(push)
; OBLIGATION 2 (ensures result >= 0) : nier -> doit être UNSAT
(assert (not (>= result 0)))
(check-sat)
(pop)
(push)
; OBLIGATION A (call-site requires allnn(p_t)) : nier -> doit être UNSAT
(assert (not (allnn p_t)))
(check-sat)
(pop)
(push)
; SOUNDNESS : un FAUX ensures `result >= 1` ne doit PAS être prouvable -> nier doit être SAT
(assert (not (>= result 1)))
(check-sat)
(pop)
