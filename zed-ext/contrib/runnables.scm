; Foundry test functions (function names starting with "test")
; Matches: function testIncrement() public { ... }
(contract_body
  (function_definition
    name: (identifier) @run
    (#match? @run "^test")
  ) @_
  (#set! tag foundry-test))

; Foundry setUp function
(contract_body
  (function_definition
    name: (identifier) @_name
    (#eq? @_name "setUp")
  ) @run @_
  (#set! tag foundry-setup))
