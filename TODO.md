# TODO

Known gaps, to cover in subsequent patches:
- [ ] Add LICENSE / NOTICE files for vendored binaries
- [ ] Implement agent auth
- [ ] Add explicit database migrations for workload schema changes
- [ ] Skip image pull when the image and snapshot are already present (or make pull policy configurable)
- [ ] Move workload storage onto a dedicated DB thread/actor instead of a shared blocking Mutex
- [ ] Properly cap / handle container output (FluentBit?) 
