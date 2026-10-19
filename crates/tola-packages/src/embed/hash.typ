// @tola/hash:0.0.0 - BLAKE3 digests and their hexadecimal spelling
//
// `blake3` hashes bytes and returns the 32-byte BLAKE3 digest. `bytes-to-hex` spells
// bytes as lowercase hexadecimal. A string hashes as its UTF-8 encoding, so pass
// `bytes(text)` when the input is text.
//
// Example - name a file after the bytes it holds:
//
// ```typst
// #import "@tola/hash:0.0.0": blake3, bytes-to-hex
// #let source = "<svg/>"
// #let output = "assets/" + bytes-to-hex(blake3(bytes(source))) + ".svg"
// ```

/// Hash bytes with BLAKE3.
///
/// The result is 32 bytes. `bytes-to-hex` spells the same digest as 64 hexadecimal
/// characters.
///
/// Example - hash the UTF-8 encoding of a string:
///
/// ```typst
/// #import "@tola/hash:0.0.0": blake3
/// #assert.eq(blake3(bytes("abc")).len(), 32)
/// ```
///
/// - value (bytes): The bytes to hash.
/// -> bytes
#let blake3(value) = {
  import "@tola/host:0.0.0": blake3 as digest
  digest(value)
}

/// Spell bytes as lowercase hexadecimal, two digits per byte.
///
/// Example:
///
/// ```typst
/// #import "@tola/hash:0.0.0": bytes-to-hex
/// #assert.eq(bytes-to-hex(bytes((1, 255))), "01ff")
/// ```
///
/// - value (bytes): The bytes to spell.
/// -> str
#let bytes-to-hex(value) = {
  import "@tola/host:0.0.0": bytes-to-hex as spell
  spell(value)
}
