// @tola/hash:0.0.0 - Hash bytes with BLAKE3 and format them as hexadecimal
//
// `blake3` hashes bytes and returns the 32-byte BLAKE3 digest. `bytes-to-hex` writes
// bytes as lowercase hexadecimal. For text, pass `bytes(text)` to hash its UTF-8 encoding.
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
/// The result is 32 bytes. `bytes-to-hex` writes the same digest as 64 hexadecimal
/// characters.
///
/// Example: Hash the UTF-8 encoding of a string:
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

/// Return lowercase hexadecimal text, two digits per byte.
///
///
/// ```typst
/// #import "@tola/hash:0.0.0": bytes-to-hex
/// #assert.eq(bytes-to-hex(bytes((1, 255))), "01ff")
/// ```
///
/// - value (bytes): The bytes to convert to hexadecimal.
/// -> str
#let bytes-to-hex(value) = {
  import "@tola/host:0.0.0": bytes-to-hex as spell
  spell(value)
}
