# Deploy on Docker Swarm

The general guide is [README.md](README.md): requirements, release channels, backups and settings. The stack file is [`deploy/swarm/openlens.yaml`](../../deploy/swarm/openlens.yaml).

OpenLens keeps its data on one local volume and is its only writer, so the stack runs **one replica on one node**, which you pick with a label. Swarm brings the restart policy, the memory limit and rolled-back updates; it does not bring a second copy of the data (see [Backups](README.md#backups)).

## Run

```sh
curl -fsSLO https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/swarm/openlens.yaml
# Edit OPENLENS_PUBLIC_URL. The 1 GiB memory limit covers about 4 M events in 90 days.
docker node update --label-add openlens=data <node>
docker stack deploy -c openlens.yaml openlens
docker service logs openlens_openlens | grep -i owner   # first start: a single-use owner link, valid 1 hour
```

The stack publishes port 8686 on the labelled node itself (`mode: host`), not through the routing mesh, so OpenLens sees the client's address: put your reverse proxy with HTTPS in front, pointed at that node ([docker.md](docker.md#https-and-reverse-proxy)) and keep the port closed to the outside. The image runs on `linux/amd64` and `linux/arm64` nodes.

What the stack sets:

- **One task at a time:** an update stops the old task before it starts the new one (`order: stop-first`), since two processes must never write the same data.
- **Rolled back by Swarm:** if the new task fails during an update, Swarm goes back to the previous image (`failure_action: rollback`).
- **A long first start:** the healthcheck has a 15-minute start period, because a new version first copies the data to `backups/upgrade/` before it listens. Failed checks before the first success do not count.
- **Memory:** a 1 GiB limit, which covers about 4 M events in 90 days ([requirements](README.md#requirements)).

## Updates

The stack follows a channel: a minor such as `:0.6` (patches only) or `:latest`, never `:edge`. Swarm pins the image to its digest at deploy time, so deploy the stack again to make it look at the tag, for example from a daily cron line (with the stack file where you keep it):

```sh
17 4 * * * docker stack deploy -c /srv/openlens/openlens.yaml openlens > /dev/null
```

`docker stack deploy` resolves the tag to its digest on every run. Do not use `docker service update --image` with the same tag for this: on current Docker it can leave the service without a digest, and from then on the same command changes nothing, so no new image ever arrives.

When the digest behind the tag has not changed, nothing restarts. When it has, Swarm replaces the task and rolls back if the new one fails. A rolled-back image is tried again on the next run, until a newer one is out. `docker service inspect openlens_openlens --format '{{.UpdateStatus.State}}'` says `completed` or `rollback_completed`.

### Roll back

By hand: `docker service rollback openlens_openlens`, or `docker service update --image ghcr.io/philippe-arnd/openlens:vX.Y.Z openlens_openlens` for a given release.

If the previous release then refuses the data (*this data was written by OpenLens X (data format N)*), the newer release changed the data format. Scale the service to 0 (`docker service scale openlens_openlens=0`), put the upgrade backup back with the BusyBox recipe of [docker.md](docker.md#roll-back) on that node (with `-v openlens_openlens-data:/data` in place of `--volumes-from`), then scale back to 1.
