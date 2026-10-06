// @tola/host:0.0.0 - the host interface Tola's other packages are built on

// Site authors import the purpose-built packages instead: `@tola/site`, `@tola/icon`, and so on.
// This package exists so exactly one file names the `sys.inputs` key holding the module Tola
// injects; the others reach it through an ordinary package import.

#import sys.inputs.at("__tola"): *
