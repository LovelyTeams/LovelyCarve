# LovelyCarve integration

Vendored core from https://github.com/SecurityRonin/veracrypt-forensic at `e3883a5b347f50b51de70ca08655b2fd4b0885b2`. Apache-2.0; original license and tests retained.

Local changes: standalone manifest; reject out-of-range/truncated encrypted data instead of synthesizing ciphertext. Application integration adds bounded evidence access and metadata validation.

VeraCrypt data geometry requires declared nonzero size and 512-byte sectors. Header search checks reader cancellation between PRF derivations. The legacy zero-size fallback test now asserts rejection. Application PIM range is 0–1000; system encryption and keyfiles are not exposed.

System/in-place encryption flags and partially encrypted data areas are rejected. The synthetic upstream fixture volume_size was corrected to the encrypted payload size to match the public cryptsetup corpus.
