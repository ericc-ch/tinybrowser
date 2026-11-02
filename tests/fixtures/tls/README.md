# Test TLS fixtures

`loopback-127.0.0.1.crt.pem` is a self-signed P-256 certificate for
`127.0.0.1`, valid 2020..2126, with `CA:TRUE` and `keyCertSign` so it verifies
itself as a chain of one. `.key.pem` is its key; it signs nothing else.

The `net` crate's TLS test and the browser's WebDriver test both dial it, so it
lives here once. Regenerate both files together:

    openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
        -keyout loopback-127.0.0.1.key.pem -out request.csr \
        -subj /CN=127.0.0.1 -addext subjectAltName=IP:127.0.0.1
    openssl x509 -req -in request.csr -signkey loopback-127.0.0.1.key.pem \
        -not_before 20200101000000Z -days 39000 \
        -extfile <(printf 'subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign\n')
