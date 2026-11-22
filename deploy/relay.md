The operator runs `tube2pod-relay` with `RELAY_DOMAIN` and a persistent, private
`RELAY_REGISTRATIONS` file. The three relay bind addresses default to loopback;
keep them private. Configure the TLS/SNI ingress in `relay-nginx.conf.example`
and supply certificates and DNS for the indicated operator-owned names.
The home process receives the operator endpoint in `TUBE2POD_RELAY_ENDPOINT`;
no inbound connection to the home network is required. Keep the registration
file across relay restarts so existing homes retain their addresses.
