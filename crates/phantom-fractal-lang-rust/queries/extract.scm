; Phantom extract query over tree-sitter-rust (SPEC-extract G2).
; The outer capture names the record kind. @visibility is optional on every
; pattern. Module-scope filtering happens in Rust, not here.

(function_item
  (visibility_modifier)? @visibility
  name: (identifier) @name) @function

(struct_item
  (visibility_modifier)? @visibility
  name: (type_identifier) @name) @struct

(enum_item
  (visibility_modifier)? @visibility
  name: (type_identifier) @name) @enum

(trait_item
  (visibility_modifier)? @visibility
  name: (type_identifier) @name) @trait

(mod_item
  (visibility_modifier)? @visibility
  name: (identifier) @name) @module

(use_declaration
  (visibility_modifier)? @visibility
  argument: (_) @argument) @use
