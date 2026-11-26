# Deploy on Docker Swarm

The general guide is [README.md](README.md): requirements, release channels, backups and settings. The stack file is [`deploy/swarm/openlens.yaml`](../../deploy/swarm/openlens.yaml).

OpenLens keeps its data on one local volume and is its only writer, so the stack runs **one replica on one node**, which you pick with a label. Swarm brings the restart policy, the memory limit and rolled-back updates; it does not bring a second copy of the data (see [Backups](README.md#backups)).

## Run

```sh
curl -fsSLO https://raw.githubusercontent.com/Philippe-arnd/OpenLens/main/deploy/swarm/openlens.yaml
# Edit OPENLENS_PUBLIC_URL, and the memory limit if you have more than about 2 M events in 90 days.
docker node update --label-add openlens=data <node>
docker stack deploy -c openlens.yaml openlens
docker service logs openlens_openlens | grep -i owner   # first start: a single-use owner link, valid 1 hour
```

The stack publishes port 8686 through the routing mesh: put your reverse proxy with HTTPS in front ([docker.md](docker.md#https-and-reverse-proxy)) and keep the port closed to the outside. The image runs on `linux/amd64` and `linux/arm64` nodes.

What the stack sets:

- **One task at a time:** an update stops the old task before it starts the new one (`order: stop-first`), since two processes must never write the same data.
- **Rolled back by Swarm:** if the new task fails during an update, Swarm goes back to the previous image (`failure_action: rollback`).
- **A long first start:** the healthcheck has a 15-minute start period, because a new version first copies the data to `backups/upgrade/` before it listens. Failed checks before the first success do not count.
- **Memory:** a 1 GiB limit, which covers about 2 M events in 90 days; 2G up to about 4 M ([requirements](README.md#requirements)).

## Updates

The stack follows a channel: a minor such as `:0.6` (patches only) or `:latest`, never `:edge`. Swarm pins the image to its digest at deploy time, so ask it to look at the tag again, for example from a daily cron line:

```sh
17 4 * * * docker service update --quiet --image ghcr.io/philippe-arnd/openlens:0.6 openlens_openlens
```

When the digest behind the tag has not changed, nothing restarts. When it has, Swarm replaces the task and rolls back if the new one fails. A rolled-back image is tried again on the next run, until a newer one is out. `docker service inspect openlens_openlens --format '{{.UpdateStatus.State}}'` says `completed` or `rollback_completed`.

### Roll back

By hand: `docker service rollback openlens_openlens`, or `docker service update --image ghcr.io/philippe-arnd/openlens:vX.Y.Z openlens_openlens` for a given release.

If the previous release then refuses the data (*this data was written by OpenLens X (data format N)*), the newer release changed the data format. Scale the service to 0 (`docker service scale openlens_openlens=0`), put the upgrade backup back with the BusyBox recipe of [docker.md](docker.md#roll-back) on that node (with `-v openlens_openlens-data:/data` in place of `--volumes-from`), then scale back to 1.
