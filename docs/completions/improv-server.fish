complete -c improv-server -l static -d 'Also serve the files in DIR (a web front end\'s build) at /, beside the API; off by default' -r -F
complete -c improv-server -l tls-cert -d 'Serve HTTPS with this certificate chain (PEM, the server\'s certificate first); needs --tls-key' -r -F
complete -c improv-server -l tls-key -d 'The private key of --tls-cert (PEM: PKCS#8, PKCS#1 or SEC1)' -r -F
complete -c improv-server -s h -l help -d 'Print help (see more with \'--help\')'
complete -c improv-server -s V -l version -d 'Print version'
