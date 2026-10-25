# Container for the listening-party relay. Build and run from this folder:
#   docker build -t ytmusic-relay .
#   docker run -p 8787:8787 ytmusic-relay
FROM denoland/deno:alpine

WORKDIR /app
COPY relay.ts .
RUN deno cache relay.ts

ENV PORT=8787
EXPOSE 8787
USER deno

# --allow-read lets the relay serve /latest.json for the app's update check; mount that
# file with  -v /path/to/latest.json:/app/latest.json:ro  (it is optional)
CMD ["run", "--allow-net", "--allow-env", "--allow-read=latest.json", "relay.ts"]
