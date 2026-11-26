; Calls: the callee expression.
(call function: [(identifier) (attribute)] @reference.call)

; Every other name or attribute chain; definition, parameter, import and assignment-target
; positions are filtered out in code (see `is_read`).
(identifier) @reference
(attribute) @reference
