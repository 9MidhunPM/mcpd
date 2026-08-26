FROM node:22-alpine AS build

WORKDIR /workspace
COPY site/package.json site/package-lock.json ./site/
RUN cd site && npm ci
COPY site ./site
COPY scripts/install.sh ./scripts/install.sh
RUN cd site && npm run build

FROM nginxinc/nginx-unprivileged:alpine

COPY --from=build /workspace/site/dist /usr/share/nginx/html
EXPOSE 8080
