# comfyui_client

A client for use with

https://github.com/nick1udwig/comfyui_provider

https://github.com/nick1udwig/provider-dao-rollup

https://github.com/nick1udwig/provider_dao_router

https://github.com/comfyanonymous/ComfyUI

## Setup

Follow instructions [here](https://github.com/nick1udwig/comfyui_provider), then:

```
# Install client (bash)
git clone https://github.com/nick1udwig/comfyui_client
kit bs comfyui_provider -p $CLIENT_PORT

# Setup rollup (rollup.os terminal)
# Setup provider (provider.os terminal)
admin:comfyui_provider:nick1udwig.os {"SetRouterProcess": {"process_id": "provider_dao_router:provider_dao_router:nick1udwig.os"}}
admin:comfyui_provider:nick1udwig.os {"SetRollupSequencer": {"address": "ROLLUP.os@sequencer:provider-dao-rollup:nick1udwig.os"}}
```

## Example usage

```
m our@client:comfyui_client:nick1udwig.os '{"RunJob": {"workflow": "workflow", "parameters": "{\"quality\": \"fast\", \"aspect_ratio\": \"square\", \"workflow\": \"workflow\", \"user_id\": \"0\", \"negative_prompt\": \"\", \"positive_prompt\": \"going for a walk in the park and looking at beautiful flowers and butterflies\", \"cfg_scale\": {\"min\": 1.0, \"max\": 1.0}, \"character\": {\"id\": \"pepe\"}, \"styler\": {\"id\": \"hand-drawn\"}}"}}'
```
