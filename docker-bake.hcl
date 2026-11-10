// Linked targets build correctly on an empty runner: no local worker tag required.
group "default" {
  targets = ["workbench"]
}

target "worker" {
  context = "."
  dockerfile = "workers/Dockerfile"
  platforms = ["linux/amd64"]
  args = { PIP_INDEX_URL = "https://pypi.org/simple" }
}

target "workbench" {
  context = "."
  dockerfile = "workers/Dockerfile.workbench"
  contexts = { "wukong-worker:0.1" = "target:toolchain" }
  platforms = ["linux/amd64"]
}

target "toolchain" {
  inherits = ["worker"]
  target = "toolchain"
}
