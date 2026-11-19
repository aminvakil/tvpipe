# Cast fixtures

- `test-cert.der`, `test-key.pk8.der`: a throwaway self-signed P-256
  certificate and its PKCS#8 key for the in-test fake Cast receiver
  (`src/cast/testing.rs`). Not a secret. Generated with OpenSSL 3:

  ```sh
  openssl ecparam -name prime256v1 -genkey -noout -out key.pem
  openssl req -new -x509 -key key.pem -out cert.pem -days 36500 \
      -subj /CN=streamlib-fake-cast -sha256 -addext "subjectAltName=IP:127.0.0.1"
  openssl x509 -in cert.pem -outform DER -out test-cert.der
  openssl pkcs8 -topk8 -nocrypt -in key.pem -outform DER -out test-key.pk8.der
  ```
