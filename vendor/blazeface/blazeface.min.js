/**
 * @license
 * Copyright 2023 Google LLC.
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 * =============================================================================
 */
!function(t,e){"object"==typeof exports&&"undefined"!=typeof module?e(exports,require("@tensorflow/tfjs-converter"),require("@tensorflow/tfjs-core")):"function"==typeof define&&define.amd?define(["exports","@tensorflow/tfjs-converter","@tensorflow/tfjs-core"],e):e((t="undefined"!=typeof globalThis?globalThis:t||self).blazeface={},t.tf,t.tf)}(this,(function(t,e,n){"use strict";function r(t){var e=Object.create(null);return t&&Object.keys(t).forEach((function(n){if("default"!==n){var r=Object.getOwnPropertyDescriptor(t,n);Object.defineProperty(e,n,r.get?r:{enumerable:!0,get:function(){return t[n]}})}})),e.default=t,Object.freeze(e)}var o=r(e),i=r(n);
/*! *****************************************************************************
    Copyright (c) Microsoft Corporation.

    Permission to use, copy, modify, and/or distribute this software for any
    purpose with or without fee is hereby granted.

    THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
    REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY
    AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
    INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
    LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR
    OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
    PERFORMANCE OF THIS SOFTWARE.
    ***************************************************************************** */
function s(t,e,n,r){return new(n||(n=Promise))((function(o,i){function s(t){try{c(r.next(t))}catch(t){i(t)}}function a(t){try{c(r.throw(t))}catch(t){i(t)}}function c(t){var e;t.done?o(t.value):(e=t.value,e instanceof n?e:new n((function(t){t(e)}))).then(s,a)}c((r=r.apply(t,e||[])).next())}))}function a(t,e){var n,r,o,i,s={label:0,sent:function(){if(1&o[0])throw o[1];return o[1]},trys:[],ops:[]};return i={next:a(0),throw:a(1),return:a(2)},"function"==typeof Symbol&&(i[Symbol.iterator]=function(){return this}),i;function a(i){return function(a){return function(i){if(n)throw new TypeError("Generator is already executing.");for(;s;)try{if(n=1,r&&(o=2&i[0]?r.return:i[0]?r.throw||((o=r.return)&&o.call(r),0):r.next)&&!(o=o.call(r,i[1])).done)return o;switch(r=0,o&&(i=[2&i[0],o.value]),i[0]){case 0:case 1:o=i;break;case 4:return s.label++,{value:i[1],done:!1};case 5:s.label++,r=i[1],i=[0];continue;case 7:i=s.ops.pop(),s.trys.pop();continue;default:if(!(o=s.trys,(o=o.length>0&&o[o.length-1])||6!==i[0]&&2!==i[0])){s=0;continue}if(3===i[0]&&(!o||i[1]>o[0]&&i[1]<o[3])){s.label=i[1];break}if(6===i[0]&&s.label<o[1]){s.label=o[1],o=i;break}if(o&&s.label<o[2]){s.label=o[2],s.ops.push(i);break}o[2]&&s.ops.pop(),s.trys.pop();continue}i=e.call(t,s)}catch(t){i=[6,t],r=0}finally{n=o=0}if(5&i[0])throw i[1];return{value:i[0]?i[1]:void 0,done:!0}}([i,a])}}}
/**
     * @license
     * Copyright 2019 Google LLC. All Rights Reserved.
     * Licensed under the Apache License, Version 2.0 (the "License");
     * you may not use this file except in compliance with the License.
     * You may obtain a copy of the License at
     *
     * https://www.apache.org/licenses/LICENSE-2.0
     *
     * Unless required by applicable law or agreed to in writing, software
     * distributed under the License is distributed on an "AS IS" BASIS,
     * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
     * See the License for the specific language governing permissions and
     * limitations under the License.
     * =============================================================================
     */var c=function(t){return{startEndTensor:t,startPoint:i.slice(t,[0,0],[-1,2]),endPoint:i.slice(t,[0,2],[-1,2])}},u=function(t,e){var n=i.mul(t.startPoint,e),r=i.mul(t.endPoint,e),o=i.concat2d([n,r],1);return c(o)},l={strides:[8,16],anchors:[2,6]};function f(t,e){var n,r,o;if(t.topLeft instanceof i.Tensor&&t.bottomRight instanceof i.Tensor){var s=i.tidy((function(){return[i.concat([i.slice(i.sub(e-1,t.topLeft),0,1),i.slice(t.topLeft,1,1)]),i.concat([i.sub(e-1,i.slice(t.bottomRight,0,1)),i.slice(t.bottomRight,1,1)])]}));n=s[0],r=s[1],null!=t.landmarks&&(o=i.tidy((function(){var n=i.sub(i.tensor1d([e-1,0]),t.landmarks),r=i.tensor1d([1,-1]);return i.mul(n,r)})))}else{var a=t.topLeft,c=a[0],u=a[1],l=t.bottomRight,f=l[0],d=l[1];n=[e-1-c,u],r=[e-1-f,d],null!=t.landmarks&&(o=t.landmarks.map((function(t){return[e-1-t[0],t[1]]})))}var h={topLeft:n,bottomRight:r};return null!=o&&(h.landmarks=o),null!=t.probability&&(h.probability=t.probability instanceof i.Tensor?t.probability.clone():t.probability),h}function d(t,e){return i.tidy((function(){var n;return n=t.hasOwnProperty("box")?t.box:t,i.squeeze(u(n,e).startEndTensor)}))}var h=function(){function t(t,e,n,r,o,s){this.blazeFaceModel=t,this.width=e,this.height=n,this.maxFaces=r,this.anchorsData=function(t,e,n){for(var r=[],o=0;o<n.strides.length;o++)for(var i=n.strides[o],s=Math.floor((e+i-1)/i),a=Math.floor((t+i-1)/i),c=n.anchors[o],u=0;u<s;u++)for(var l=i*(u+.5),f=0;f<a;f++)for(var d=i*(f+.5),h=0;h<c;h++)r.push([d,l]);return r}(e,n,l),this.anchors=i.tensor2d(this.anchorsData),this.inputSizeData=[e,n],this.inputSize=i.tensor1d([e,n]),this.iouThreshold=o,this.scoreThreshold=s}return t.prototype.getBoundingBoxes=function(t,e,n){return void 0===n&&(n=!0),s(this,void 0,void 0,(function(){var r,o,u,l,f,d,h,p,b,v,m,y,w,g,x=this;return a(this,(function(z){switch(z.label){case 0:return r=i.tidy((function(){var e,n,r,o,s,a,c,u,l,f,d,h,p,b=i.image.resizeBilinear(t,[x.width,x.height]),v=i.mul(i.sub(i.div(b,255),.5),2),m=x.blazeFaceModel.predict(v),y=i.squeeze(m),w=(e=y,n=x.anchors,r=x.inputSize,o=i.slice(e,[0,1],[-1,2]),s=i.add(o,n),a=i.slice(e,[0,3],[-1,2]),c=i.div(a,r),u=i.div(s,r),l=i.div(c,2),f=i.sub(u,l),d=i.add(u,l),h=i.mul(f,r),p=i.mul(d,r),i.concat2d([h,p],1)),g=i.slice(y,[0,0],[-1,1]);return[y,w,i.squeeze(i.sigmoid(g))]})),o=r[0],u=r[1],l=r[2],f=console.warn,console.warn=function(){},d=i.image.nonMaxSuppression(u,l,this.maxFaces,this.iouThreshold,this.scoreThreshold),console.warn=f,[4,d.array()];case 1:return h=z.sent(),d.dispose(),p=h.map((function(t){return i.slice(u,[t,0],[1,-1])})),e?[3,3]:[4,Promise.all(p.map((function(t){return s(x,void 0,void 0,(function(){var e;return a(this,(function(n){switch(n.label){case 0:return[4,t.array()];case 1:return e=n.sent(),t.dispose(),[2,e]}}))}))})))];case 2:p=z.sent(),z.label=3;case 3:for(b=t.shape[1],v=t.shape[2],m=e?i.div([v,b],this.inputSize):[v/this.inputSizeData[0],b/this.inputSizeData[1]],y=[],w=function(t){var r=p[t],s=i.tidy((function(){var s=r instanceof i.Tensor?c(r):c(i.tensor2d(r));if(!n)return s;var a,u=h[t];return a=e?i.slice(x.anchors,[u,0],[1,2]):x.anchorsData[u],{box:s,landmarks:i.reshape(i.squeeze(i.slice(o,[u,5],[1,-1])),[6,-1]),probability:i.slice(l,[u],[1]),anchor:a}}));y.push(s)},g=0;g<p.length;g++)w(g);return u.dispose(),l.dispose(),o.dispose(),[2,{boxes:y,scaleFactor:m}]}}))}))},t.prototype.estimateFaces=function(t,e,n,r){return void 0===e&&(e=!1),void 0===n&&(n=!1),void 0===r&&(r=!0),s(this,void 0,void 0,(function(){var o,c,u,l,h,p,b=this;return a(this,(function(v){switch(v.label){case 0:return o=function(t){return t instanceof i.Tensor?[t.shape[0],t.shape[1]]:[t.height,t.width]}(t),c=o[1],u=i.tidy((function(){return t instanceof i.Tensor||(t=i.browser.fromPixels(t)),i.expandDims(i.cast(t,"float32"),0)})),[4,this.getBoundingBoxes(u,e,r)];case 1:return l=v.sent(),h=l.boxes,p=l.scaleFactor,u.dispose(),e?[2,h.map((function(t){var e=d(t,p),o={topLeft:i.slice(e,[0],[2]),bottomRight:i.slice(e,[2],[2])};if(r){var s=t,a=s.landmarks,u=s.probability,l=s.anchor,h=i.mul(i.add(a,l),p);o.landmarks=h,o.probability=u}return n&&(o=f(o,c)),o}))]:[2,Promise.all(h.map((function(t){return s(b,void 0,void 0,(function(){var e,o,i,u,l,h,b,v,m,y,w,g=this;return a(this,(function(x){switch(x.label){case 0:return e=d(t,p),r?[3,2]:[4,e.array()];case 1:return l=x.sent(),o={topLeft:l.slice(0,2),bottomRight:l.slice(2)},[3,4];case 2:return[4,Promise.all([t.landmarks,e,t.probability].map((function(t){return s(g,void 0,void 0,(function(){return a(this,(function(e){return[2,t.array()]}))}))})))];case 3:i=x.sent(),u=i[0],l=i[1],h=i[2],b=t.anchor,m=(v=p)[0],y=v[1],w=u.map((function(t){return[(t[0]+b[0])*m,(t[1]+b[1])*y]})),o={topLeft:l.slice(0,2),bottomRight:l.slice(2),landmarks:w,probability:h},(z=t.box).startEndTensor.dispose(),z.startPoint.dispose(),z.endPoint.dispose(),t.landmarks.dispose(),t.probability.dispose(),x.label=4;case 4:return e.dispose(),n&&(o=f(o,c)),[2,o]}var z}))}))})))]}}))}))},t.prototype.dispose=function(){this.blazeFaceModel.dispose(),this.anchors.dispose(),this.inputSize.dispose()},t}(),p="https://tfhub.dev/tensorflow/tfjs-model/blazeface/1/default/1";
/**
     * @license
     * Copyright 2019 Google LLC. All Rights Reserved.
     * Licensed under the Apache License, Version 2.0 (the "License");
     * you may not use this file except in compliance with the License.
     * You may obtain a copy of the License at
     *
     * https://www.apache.org/licenses/LICENSE-2.0
     *
     * Unless required by applicable law or agreed to in writing, software
     * distributed under the License is distributed on an "AS IS" BASIS,
     * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
     * See the License for the specific language governing permissions and
     * limitations under the License.
     * =============================================================================
     */t.BlazeFaceModel=h,t.load=function(t){var e=void 0===t?{}:t,n=e.maxFaces,r=void 0===n?10:n,i=e.inputWidth,c=void 0===i?128:i,u=e.inputHeight,l=void 0===u?128:u,f=e.iouThreshold,d=void 0===f?.3:f,b=e.scoreThreshold,v=void 0===b?.75:b,m=e.modelUrl;return s(this,void 0,void 0,(function(){var t;return a(this,(function(e){switch(e.label){case 0:return null==m?[3,2]:[4,o.loadGraphModel(m)];case 1:return t=e.sent(),[3,4];case 2:return[4,o.loadGraphModel(p,{fromTFHub:!0})];case 3:t=e.sent(),e.label=4;case 4:return[2,new h(t,c,l,r,d,v)]}}))}))}}));
//# sourceMappingURL=blazeface.min.umd.js.map
