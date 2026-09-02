# HQ

## Next Up

### Smaller hand-added todos (each need some in-depth analysis)

* The "Command approval needed" dialog does not properly handle vim navigation (k/j for up/down).
* We should have a Config page that is a sibling to the top nav elements (Inbox...Projects) that allows direct editing of all configuration values. Theme updates should take effect in real time. For theme config, it should show all supported themes. If this feature doesn't exist yet (named themes) then let's plan that out after implementing the Config page scaffolding. Things we should be able to configure: default settings for codex (like yolo, and model selection - for now this can be raw text or nothing for default).
* Let's apply syntax highlighting to the agentic command display. Like, when an agent shows us what shell command it is running conversation view, let's find a good rust library for shell syntax highlighting, and apply that to the presentation of that display element. Ideally we find one that can apply our theme/style color choices. We may have to make a mapping from our color scheme to the various semantic layers of whatever library we choose.
