# pass3d-pool
## Mining client for the pool Node
This standalone application represents a mining client for the  [3DPass Node](https://github.com/3Dpass/3DP) running in the pool mode. The mining pool mode, while being turned on (pass3d-pool is connected), enables the decentralized mining pool pallet, which will distribute mining block rewards directly among the mining pool's members and charge the mining pool fee that is set up by the pool's admin.

In order to prove of the miner's work there is an additional off-chain difficulty number being leveraged by the client app and verified on the pool Node's side. The additional difficulty is set up by the pool's admin. Every 10 sec the client app is requesting the pool Node (via the [RPC API](https://github.com/3Dpass/3DP/wiki/RPC-API-mining-pool-interaction)) for some necessary metadata, such as:

- Pub key for the client authorization and objects encryption
- Current network difficulty
- Additional off-chain difficulty
- Best block hash
- Parent block hash

The app receives 3D models from miner (3D object generator being used for mining) through the `port 9833`. Once the object corresponding the additional difficulty found, it's being sent over to the pool Node the miner's account (address) is registered in. The pool Node is available by its `--url`.

There is a statistic report being saved by the pool Node every 20 blocks on the chain storage, which is available for everyone. Once the block is mined by the mining pool Node and accepted by the network, the block rewards are being distributed directly in proportion to the input hash-rate provided by each miner in the pool. 

<img width="686" alt="pass3d-pool" src="https://user-images.githubusercontent.com/107915078/223340542-41f6a37c-3647-4cd0-9bdd-e7fa571169e7.png">


## Build
```
cargo build --release
```
## Run
```
./target/release/pass3d-pool run --pool-id <POOL's P3D ADDRESS> --url http://1.2.3.4:9933 --member-id <MINER'S P3D ADDRESS> --key <MINER's PRIVATE KEY> --threads 32
```
- `--threads` is the amount of threads being exploited for 3D objects handling
- `--url` is the pool server ip/host to connect to
- `--key` is the private key for the member's P3D address, which is being used for signing messages and member authentication.

### Inspect
Inspect a seed phrase to get the private key:
```
./target/release/pass3d-pool inspect --seed 'one two ... twelve'
```
- `--seed` is the seed phrase for your P3D address

### Parameters
```
./target/release/pass3d-pool --help
```
