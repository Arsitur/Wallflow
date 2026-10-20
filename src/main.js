import './app.css';
import App from './App.svelte';
import Logs from './Logs.svelte';
import Settings from './Settings.svelte';
import { installDiagnostics } from './lib/diagnostics.js';
installDiagnostics();
const params = new URLSearchParams(location.search);
const View = params.has('settings') ? Settings : params.has('logs') ? Logs : App;
new View({ target: document.getElementById('app') });
